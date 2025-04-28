use crate::api::messages::{Message as ApiMessage, MessageContent as ApiMessageContent};
use crate::api::{Client as ApiClient, Model};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid;

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
pub use conversation::{Conversation, Message, MessageContentBlock, Role, ToolCall};
pub use environment::EnvironmentInfo;
pub use tool_executor::ToolExecutor;

use crate::config::LlmConfig;

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
    Error {
        message: String,
    },
}

pub struct AppConfig {
    pub llm_config: LlmConfig,
}

use std::collections::{HashSet /* , HashMap */};

pub struct App {
    pub config: AppConfig,
    pub conversation: Arc<Mutex<Conversation>>,
    pub env_info: EnvironmentInfo,
    pub tool_executor: Arc<ToolExecutor>,
    pub api_client: ApiClient,
    event_sender: mpsc::Sender<AppEvent>,
    approved_tools: HashSet<String>,
    current_op_context: Option<OpContext>,
}

impl App {
    pub fn new(config: AppConfig, event_tx: mpsc::Sender<AppEvent>) -> Result<Self> {
        let env_info = EnvironmentInfo::collect()?;
        let conversation = Arc::new(Mutex::new(Conversation::new()));
        let tool_executor = Arc::new(ToolExecutor::new());
        let api_client = ApiClient::new(&config.llm_config);

        Ok(Self {
            config,
            conversation,
            env_info,
            tool_executor,
            api_client,
            event_sender: event_tx,
            approved_tools: HashSet::new(),
            current_op_context: None,
        })
    }

    pub(crate) fn emit_event(&self, event: AppEvent) {
        match self.event_sender.try_send(event.clone()) {
            Ok(_) => {
                crate::utils::logging::debug("app.emit_event", &format!("Sent event: {:?}", event));
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                crate::utils::logging::warn(
                    "app.emit_event",
                    &format!("Event channel full, discarding event: {:?}", event),
                );
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                crate::utils::logging::warn(
                    "app.emit_event",
                    &format!("Event channel closed, discarding event: {:?}", event),
                );
            }
        }
    }

    pub async fn add_message(&self, message: Message) {
        let mut conversation_guard = self.conversation.lock().await;
        conversation_guard.messages.push(message.clone());
        drop(conversation_guard);

        if message.role != Role::Tool {
            self.emit_event(AppEvent::MessageAdded {
                role: message.role,
                content_blocks: message.content_blocks.clone(),
                id: message.id,
            });
        }
    }

    pub async fn process_user_message(&mut self, message: String) -> Result<()> {
        if message.starts_with('/') {
            let response = self.handle_command(&message).await?;
            self.emit_event(AppEvent::CommandResponse {
                content: response.clone(),
                id: uuid::Uuid::new_v4().to_string(),
            });
            return Ok(());
        }

        // Cancel any existing operations
        self.cancel_current_processing().await;

        // Create a new operation context
        let op_context = OpContext::new();
        self.current_op_context = Some(op_context);

        // Add user message
        self.add_message(Message::new_text(Role::User, message.clone()))
            .await;

        // Start thinking and spawn API call
        self.emit_event(AppEvent::ThinkingStarted);
        // Spawn the API call instead of awaiting handle_response directly
        if let Err(e) = self.spawn_api_call().await {
            crate::utils::logging::error(
                "App.process_user_message",
                &format!("Error spawning API call task: {}", e),
            );
            self.emit_event(AppEvent::ThinkingCompleted); // Stop thinking on spawn error
            self.emit_event(AppEvent::Error {
                message: format!("Failed to start API call: {}", e),
            });
            self.current_op_context = None; // Clean up context
            return Err(e);
        }

        Ok(())
    }

    // Refactored function to spawn the API call task and add it to OpContext JoinSet
    async fn spawn_api_call(&mut self) -> Result<()> {
        crate::utils::logging::debug(
            "app.spawn_api_call",
            "Spawning API call task into JoinSet...",
        );

        let api_client = self.api_client.clone();
        let conversation = self.conversation.clone();
        // Get tools from the executor and convert them to api::Tool structs
        let api_tools = self.tool_executor.to_api_tools();
        let tools = if api_tools.is_empty() {
            None
        } else {
            Some(api_tools)
        };

        // Get mutable access to OpContext and its token
        let op_context = match &mut self.current_op_context {
            Some(ctx) => ctx,
            None => {
                let err = anyhow::anyhow!("No operation context available to spawn API call");
                crate::utils::logging::error("App.spawn_api_call", &err.to_string());
                // Ensure thinking stops if context is missing
                self.emit_event(AppEvent::ThinkingCompleted);
                return Err(err);
            }
        };

        let token = op_context.cancel_token.clone();
        // Mark that an API call is now in progress within this context
        op_context.start_api_call();

        // Clone env_info *before* the async move block
        let env_info_clone = self.env_info.clone();

        // Spawn the task directly into the OpContext's JoinSet
        op_context.tasks.spawn(async move {
            crate::utils::logging::debug("spawn_api_call task (JoinSet)", "Task started.");

            let response_result = App::get_claude_response_static(
                conversation,
                api_client,
                tools.as_ref(),
                token,
                &env_info_clone,
            )
            .await;

            crate::utils::logging::debug(
                "spawn_api_call task (JoinSet)",
                &format!(
                    "API call finished with result: {:?}",
                    response_result.is_ok()
                ),
            );

            // Return TaskOutcome::ApiResponse
            TaskOutcome::ApiResponse {
                result: response_result.map_err(|e| e.to_string()),
            }
        });

        crate::utils::logging::debug(
            "app.spawn_api_call",
            "API call task successfully spawned into JoinSet.",
        );

        Ok(())
    }

    pub async fn handle_tool_command_response(
        &mut self,
        tool_call_id: String,
        approved: bool,
        always: bool,
    ) -> Result<()> {
        crate::utils::logging::debug(
            "App.handle_tool_command_response",
            &format!(
                "Handling response for tool call ID: {}, Approved: {}, Always: {}",
                tool_call_id, approved, always
            ),
        );

        // Get op_context mutably
        let tool_call_id_clone = tool_call_id.clone(); // Clone ID for potential removal
        let mut tool_call_to_execute: Option<crate::api::ToolCall> = None;
        let mut token_for_spawn: Option<CancellationToken> = None;
        let mut denied = false;
        let mut denial_result_content: Option<String> = None;
        let mut denial_should_continue = false;
        let mut tool_name_for_approval: Option<String> = None;

        if let Some(ctx) = self.current_op_context.as_mut() {
            if let Some(tool_call) = ctx.pending_tool_calls.remove(&tool_call_id_clone) {
                let tool_name = tool_call.name.clone();
                tool_name_for_approval = Some(tool_name.clone());

                if approved {
                    crate::utils::logging::info(
                        "App.handle_tool_command_response",
                        &format!("Tool call '{}' approved.", tool_name),
                    );

                    tool_call_to_execute = Some(tool_call.clone());
                    ctx.expected_tool_results += 1;
                    token_for_spawn = Some(ctx.cancel_token.clone());
                } else {
                    crate::utils::logging::info(
                        "App.handle_tool_command_response",
                        &format!("Tool call '{}' was denied by the user.", tool_name),
                    );
                    let result_content = format!("Tool '{}' denied by user.", tool_name);

                    // Mark as denied and store info needed after borrow
                    denied = true;
                    denial_result_content = Some(result_content);
                    // Check if we should continue after denial
                    denial_should_continue =
                        ctx.pending_tool_calls.is_empty() && ctx.expected_tool_results == 0;
                }
            } else {
                // Tool not found in pending calls
                crate::utils::logging::warn(
                    "App.handle_tool_command_response",
                    &format!(
                        "Received response for unknown or already handled tool call ID: {}",
                        tool_call_id
                    ),
                );
                return Ok(()); // Early return if tool not found
            }
        } else {
            // No OpContext found
            let err = anyhow::anyhow!("No operation context available for tool approval/denial");
            crate::utils::logging::error("App.handle_tool_command_response", &err.to_string());
            return Err(err);
        }

        // --- Post-Borrow Handling ---

        // If approved and 'always' is true, add to approved_tools set
        if approved && always {
            if let Some(tool_name) = &tool_name_for_approval {
                crate::utils::logging::debug(
                    "App.handle_tool_command_response",
                    &format!("Adding tool '{}' to always-approved list.", tool_name),
                );
                self.approved_tools.insert(tool_name.clone());
            }
        }

        if denied {
            // Handle denial actions after borrow is released
            if let Some(result_content) = denial_result_content {
                // Retrieve tool name from context again if possible, or use ID
                let tool_name = self
                    .current_op_context
                    .as_ref()
                    .and_then(|ctx| ctx.pending_tool_calls.get(&tool_call_id))
                    .map(|tc| tc.name.clone())
                    .unwrap_or_else(|| "unknown_tool".to_string()); // Fallback needed

                // Add result to conversation
                self.conversation
                    .lock()
                    .await
                    .add_tool_result(tool_call_id.clone(), result_content.clone());
                // Emit event
                self.emit_event(AppEvent::ToolCallFailed {
                    name: tool_name,
                    error: "Denied by user".to_string(),
                    id: tool_call_id.clone(),
                });

                if denial_should_continue {
                    crate::utils::logging::info(
                        "App.handle_tool_command_response",
                        "All tools handled after denial. Continuing operation.",
                    );
                    // Call continue_operation_after_tools
                    self.continue_operation_after_tools().await?;
                }
            }
            return Ok(()); // Finish after handling denial
        }

        // --- Spawning logic (if approved) ---
        if let (Some(tool_call), Some(token)) = (tool_call_to_execute, token_for_spawn) {
            let tool_call_id_for_log = tool_call.id.clone();
            let tool_name_for_log = tool_call.name.clone();
            let tool_executor = self.tool_executor.clone();

            // Get op_context mutably for spawning
            let op_context = self.current_op_context.as_mut().unwrap();
            op_context.add_active_tool(tool_call_id_for_log.clone(), tool_name_for_log.clone());

            // Capture necessary values for spawn
            let captured_tool_name = tool_name_for_log.clone();
            op_context.tasks.spawn(async move {
                let task_id = tool_call.id.clone();
                // Map ToolError to anyhow::Error
                let result: Result<String, anyhow::Error> =
                    context_util::execute_tool_task_logic(tool_call.clone(), tool_executor, token)
                        .await
                        .map_err(|e| anyhow::anyhow!(e)); // Convert ToolError -> anyhow::Error

                // Construct TaskOutcome::ToolResult
                TaskOutcome::ToolResult {
                    tool_call_id: task_id,
                    tool_name: captured_tool_name,
                    result, // Now Result<String, anyhow::Error>
                }
            });

            // Emit ToolCallStarted event AFTER spawning
            self.emit_event(AppEvent::ToolCallStarted {
                name: tool_name_for_log.clone(),
                id: tool_call_id_for_log.clone(),
            });

            crate::utils::logging::debug(
                "App.handle_tool_command_response",
                &format!(
                    "Spawned task for approved tool '{}' (ID: {}) into JoinSet",
                    tool_name_for_log, tool_call_id_for_log
                ),
            );
        }

        Ok(())
    }

    async fn initiate_tool_calls(&mut self, tool_calls: Vec<crate::api::ToolCall>) -> Result<()> {
        if tool_calls.is_empty() {
            crate::utils::logging::debug("App.initiate_tool_calls", "No tool calls to initiate.");
            return Ok(());
        }

        // Get a mutable reference to the operation context, error if missing
        let op_context = match &mut self.current_op_context {
            Some(ctx) => ctx,
            None => {
                let err = anyhow::anyhow!("No operation context available for tool execution");
                crate::utils::logging::error("App.initiate_tool_calls", &err.to_string());
                self.emit_event(AppEvent::ThinkingCompleted); // Assume thinking stops if context is lost
                return Err(err);
            }
        };

        let cancel_token_clone = op_context.cancel_token.clone(); // Clone token from mutable borrow

        let mut tools_to_execute = Vec::new();
        let mut tools_needing_approval = Vec::new();

        crate::utils::logging::info(
            "App.initiate_tool_calls",
            &format!("Initiating {} tool calls.", tool_calls.len()),
        );

        // Process each tool call
        for tool_call in tool_calls {
            let tool_name = tool_call.name.clone();
            let tool_id = tool_call.id.clone();

            let tool_call_with_id = crate::api::ToolCall {
                id: tool_id.clone(),
                ..tool_call
            };

            // Check if tool requires approval by default
            let requires_approval = self.tool_executor.requires_approval(&tool_name)?;

            // Check approved tools (immutable borrow of self needed, but that's fine here)
            let is_approved = !requires_approval || self.approved_tools.contains(&tool_name);

            if is_approved {
                crate::utils::logging::debug(
                    "App.initiate_tool_calls",
                    &format!(
                        "Tool '{}' is {}approved, adding to execution list.",
                        tool_name,
                        if requires_approval {
                            "requires approval and "
                        } else {
                            "already "
                        }
                    ),
                );
                tools_to_execute.push(tool_call_with_id);
            } else {
                crate::utils::logging::debug(
                    "App.initiate_tool_calls",
                    &format!("Tool '{}' needs approval.", tool_name),
                );
                // Store in the OpContext's pending_tool_calls
                // `op_context` is already mutably borrowed
                op_context
                    .pending_tool_calls
                    .insert(tool_id.clone(), tool_call_with_id.clone());
                tools_needing_approval.push(tool_call_with_id);
            }
        }

        // Request approval for tools that need it
        let approval_requests: Vec<_> = tools_needing_approval
            .iter()
            .map(|tc| (tc.name.clone(), tc.parameters.clone(), tc.id.clone()))
            .collect();

        // Emit approval request events *after* processing all tool calls in the list
        // This releases the mutable borrow of op_context before we borrow self immutably for emit_event
        let _ = op_context; // Explicitly end mutable borrow before immutable borrow for emit_event

        for (name, parameters, id) in approval_requests {
            crate::utils::logging::debug(
                "App.initiate_tool_calls",
                &format!("Requesting approval for tool: {}", name),
            );
            self.emit_event(AppEvent::RequestToolApproval {
                name,
                parameters,
                id,
            });
        }

        // Vector to store data for events to be emitted later
        let mut tool_started_events_data: Vec<(String, String)> = Vec::new();

        // Execute approved tools (if any)
        if !tools_to_execute.is_empty() {
            crate::utils::logging::debug(
                "App.initiate_tool_calls",
                &format!(
                    "Executing {} already approved tools.",
                    tools_to_execute.len()
                ),
            );

            // Re-borrow mutably to modify expected_tool_results and spawn tasks
            let op_context = self.current_op_context.as_mut().unwrap(); // Assume context exists now
            op_context.expected_tool_results += tools_to_execute.len(); // Increment expected

            // Iterate over a slice to avoid moving the vector
            for tool_call_ref in &tools_to_execute {
                // Clone the necessary data *outside* the async block
                let tool_call_owned = tool_call_ref.clone();
                let tool_id = tool_call_owned.id.clone();
                let tool_name = tool_call_owned.name.clone();

                // Add active tool tracking *before* spawning
                op_context.add_active_tool(tool_id.clone(), tool_name.clone());

                // Prepare args for spawn that need cloning
                let tool_executor = self.tool_executor.clone();
                let token = cancel_token_clone.clone();

                // Store event data instead of emitting immediately
                tool_started_events_data.push((tool_name.clone(), tool_id.clone()));

                // Spawn the task, moving the owned data
                op_context.tasks.spawn(async move {
                    // tool_call_owned is moved into this block
                    let task_id = tool_call_owned.id.clone(); // Clone ID from owned data
                    let tool_name_captured = tool_call_owned.name.clone(); // Clone name from owned data

                    let result: Result<String, anyhow::Error> =
                        context_util::execute_tool_task_logic(
                            tool_call_owned, // Pass the owned ToolCall
                            tool_executor,
                            token,
                        )
                        .await
                        .map_err(|e| anyhow::anyhow!(e)); // Convert ToolError -> anyhow::Error

                    // Return TaskOutcome::ToolResult
                    TaskOutcome::ToolResult {
                        tool_call_id: task_id,
                        tool_name: tool_name_captured,
                        result, // Now Result<String, anyhow::Error>
                    }
                });

                crate::utils::logging::debug(
                    "App.initiate_tool_calls",
                    &format!(
                        "Spawned task for tool '{}' (ID: {}) into JoinSet",
                        tool_name, // Use name cloned outside
                        tool_id    // Use id cloned outside
                    ),
                );
            }
            // Mutable borrow of op_context ends here
        }

        // Emit events AFTER the mutable borrow for op_context is released
        for (name, id) in tool_started_events_data {
            self.emit_event(AppEvent::ToolCallStarted { name, id });
        }

        // Check completion status (only if no tools were spawned AND none need approval)
        // This logic needs refinement based on the actor loop polling
        if tools_to_execute.is_empty() && tools_needing_approval.is_empty() {
            crate::utils::logging::debug(
                "App.initiate_tool_calls",
                "No tools needed approval and none were executed immediately.",
            );
            // Let the actor loop polling determine completion/ThinkingCompleted
        } else if tools_to_execute.is_empty() {
            crate::utils::logging::debug(
                "App.initiate_tool_calls",
                "No tools were immediately ready for execution (all need approval). Waiting for user.",
            );
        }

        Ok(())
    }

    pub async fn dispatch_agent(&self, prompt: &str, token: CancellationToken) -> Result<String> {
        let agent =
            crate::tools::dispatch_agent::DispatchAgent::with_api_client(self.api_client.clone());
        agent.execute(prompt, token).await
    }

    pub async fn handle_command(&mut self, command: &str) -> Result<String> {
        let parts: Vec<&str> = command.trim_start_matches('/').splitn(2, ' ').collect();
        let command_name = parts[0];
        let args = parts.get(1).unwrap_or(&"").trim();

        // Cancel any previous operation before starting a command
        self.cancel_current_processing().await;

        let result: Result<String>; // Store result to allow context clearing

        match command_name {
            "clear" => {
                // clear does not need an op context
                self.conversation.lock().await.clear();
                result = Ok("Conversation cleared.".to_string());
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

                // Emit thinking started? Maybe not for commands...
                match self.compact_conversation(token).await {
                    Ok(()) => {
                        result = Ok("Conversation compacted.".to_string());
                    }
                    Err(e) => {
                        if e.to_string() == "Request cancelled" {
                            crate::utils::logging::info(
                                "App.handle_command",
                                "Compact command cancelled.",
                            );
                            result = Ok("Compact command cancelled.".to_string());
                        } else {
                            crate::utils::logging::error(
                                "App.handle_command",
                                &format!("Error during compact: {}", e),
                            );
                            result = Err(e);
                        }
                    }
                }
                self.current_op_context = None; // Clear context after command
            }
            "dispatch" => {
                if args.is_empty() {
                    return Ok("Usage: /dispatch <prompt for agent>".to_string());
                }
                // Create OpContext for cancellable command
                let op_context = OpContext::new();
                self.current_op_context = Some(op_context);
                let token = self
                    .current_op_context
                    .as_ref()
                    .unwrap()
                    .cancel_token
                    .clone();

                // Emit thinking started? Maybe not for commands...
                match self.dispatch_agent(args, token).await {
                    Ok(response) => {
                        self.add_message(Message::new_text(
                            Role::Assistant,
                            format!("Dispatch Agent Result:\\\\n{}", response),
                        ))
                        .await;
                        result = Ok(format!("Dispatch agent executed. Response added."));
                    }
                    Err(e) => {
                        if e.to_string() == "Request cancelled" {
                            crate::utils::logging::info(
                                "App.handle_command",
                                "Dispatch command cancelled.",
                            );
                            result = Ok("Dispatch command cancelled.".to_string());
                        } else {
                            crate::utils::logging::error(
                                "App.handle_command",
                                &format!("Error during dispatch: {}", e),
                            );
                            result = Err(e);
                        }
                    }
                }
                self.current_op_context = None; // Clear context after command
            }
            _ => result = Ok(format!("Unknown command: {}", command_name)),
        }

        result // Return the stored result
    }

    // TODO: Provide a CancellationToken here? Commands are not currently tied to an OpContext.
    pub async fn compact_conversation(&mut self, token: CancellationToken) -> Result<()> {
        crate::utils::logging::info("App.compact_conversation", "Compacting conversation...");

        // Create a dummy token for now as the call site doesn't have one
        // let token = CancellationToken::new();

        // Need to get client and conversation separately to avoid double borrow
        let client = self.api_client.clone();
        let conversation_arc = self.conversation.clone();

        {
            let mut conversation_guard = conversation_arc.lock().await;
            // This adds to memory, which is fine
            let _current_summary =
                format!("Conversation up to now:\n{:?}", conversation_guard.messages);

            // Call compact on the conversation guard, passing the ApiClient
            conversation_guard.compact(&client, token).await?;
        }

        crate::utils::logging::info("App.compact_conversation", "Conversation compacted.");
        Ok(())
    }

    pub async fn cancel_current_processing(&mut self) {
        // Use operation context for cancellation if available
        if let Some(mut op_context) = self.current_op_context.take() {
            crate::utils::logging::info(
                "App.cancel_current_processing",
                "Cancelling current operation via OpContext",
            );

            // Capture the current state for the cancellation info
            let active_tools = op_context.active_tools.values().cloned().collect();
            let cancellation_info = CancellationInfo {
                api_call_in_progress: op_context.api_call_in_progress,
                active_tools,
                pending_tool_approvals: !op_context.pending_tool_calls.is_empty(),
            };

            op_context.cancel_and_shutdown().await;

            self.emit_event(AppEvent::OperationCancelled {
                info: cancellation_info,
            });
            return;
        }

        crate::utils::logging::warn(
            "App.cancel_current_processing",
            "Attempted to cancel processing, but no active operation context was found.",
        );
    }

    async fn continue_operation_after_tools(&mut self) -> Result<()> {
        crate::utils::logging::info(
            "App.continue_operation_after_tools",
            "All tools completed, continuing operation with next API call",
        );

        // Ensure context exists
        if self.current_op_context.is_none() {
            crate::utils::logging::error(
                "App.continue_operation_after_tools",
                "No operation context found to continue operation.",
            );
            self.emit_event(AppEvent::ThinkingCompleted); // Stop spinner
            return Err(anyhow::anyhow!(
                "Cannot continue operation without context."
            ));
        }

        // Mark API call starting in context
        self.current_op_context.as_mut().unwrap().start_api_call();

        // Start thinking and spawn the *next* API call
        self.emit_event(AppEvent::ThinkingStarted);
        if let Err(e) = self.spawn_api_call().await {
            crate::utils::logging::error(
                "App.continue_operation_after_tools",
                &format!("Error spawning subsequent API call task: {}", e),
            );
            self.emit_event(AppEvent::ThinkingCompleted);
            self.emit_event(AppEvent::Error {
                message: format!("Failed to continue operation: {}", e),
            });
            self.current_op_context = None; // Clean up context
            return Err(e);
        }

        Ok(())
    }

    // Needs to be static or refactored
    async fn get_claude_response_static(
        conversation: Arc<Mutex<Conversation>>,
        api_client: ApiClient,
        tools: Option<&Vec<crate::api::Tool>>,
        token: CancellationToken,
        env_info: &EnvironmentInfo,
    ) -> Result<crate::api::CompletionResponse> {
        let conversation_guard = conversation.lock().await;
        let (api_messages, system_content_override) =
            crate::api::messages::convert_conversation(&conversation_guard);
        drop(conversation_guard);

        // Generate the main system prompt
        let system_prompt = create_system_prompt(env_info);

        // Combine the generated prompt with any override from the conversation
        // Prioritize the override if it exists and is not empty
        let final_system_content = system_content_override
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                if let ApiMessageContent::Text { content } = system_prompt.content {
                    if content.trim().is_empty() {
                        None
                    } else {
                        Some(content)
                    }
                } else {
                    crate::utils::logging::warn(
                        "App.get_claude_response_static",
                        "Generated system prompt was not Text content.",
                    );
                    None
                }
            });

        api_client
            .complete(
                Model::Claude3_7Sonnet20250219,
                api_messages,
                final_system_content,
                tools.cloned(),
                token,
            )
            .await
    }
}

// Define the App actor loop function
pub async fn app_actor_loop(mut app: App, mut command_rx: mpsc::Receiver<AppCommand>) {
    crate::utils::logging::info("app_actor_loop", "App actor loop started.");
    loop {
        tokio::select! {
            // Handle incoming commands
            Some(command) = command_rx.recv() => {
                crate::utils::logging::debug("app_actor_loop", &format!("Received command: {:?}", command));
                match command {
                    AppCommand::ProcessUserInput(message) => {
                        if let Err(e) = app.process_user_message(message).await {
                            crate::utils::logging::error("app_actor_loop", &format!("Error processing user message: {}", e));
                            // Error event is emitted within process_user_message
                        }
                    }
                    AppCommand::HandleToolResponse { id, approved, always } => {
                        if let Err(e) = app.handle_tool_command_response(id, approved, always).await {
                            crate::utils::logging::error("app_actor_loop", &format!("Error handling tool response: {}", e));
                            // Emit error event
                            app.emit_event(AppEvent::Error { message: format!("Failed to handle tool approval: {}", e) });
                            // If handling fails, we might lose context. Ensure spinner stops.
                            app.emit_event(AppEvent::ThinkingCompleted);
                            app.current_op_context = None; // Clear potentially inconsistent context
                        }
                    }
                    AppCommand::CancelProcessing => {
                        app.cancel_current_processing().await;
                    }
                    AppCommand::ExecuteCommand(cmd) => {
                        match app.handle_command(&cmd).await {
                            Ok(response) => {
                                // Commands might not always generate immediate user-visible output
                                // For now, maybe log or send a CommandResponse event if needed?
                                crate::utils::logging::info("app_actor_loop", &format!("Command '{}' executed, result: {}", cmd, response));
                                // Example: Send response back (adjust as needed)
                                // app.emit_event(AppEvent::CommandResponse { content: response, id: format!("cmd_resp_{}", uuid::Uuid::new_v4()) });
                            }
                            Err(e) => {
                                crate::utils::logging::error("app_actor_loop", &format!("Error running command '{}': {}", cmd, e));
                                app.emit_event(AppEvent::Error { message: format!("Command failed: {}", e) });
                            }
                        }
                    }
                    AppCommand::Shutdown => {
                        crate::utils::logging::info("app_actor_loop", "Received Shutdown command. Shutting down.");
                        // Perform any necessary cleanup before exiting
                        app.cancel_current_processing().await; // Cancel anything ongoing
                        break; // Exit the loop
                    }
                }
            }

            // Poll for completed tasks (Tools, API calls) from OpContext
            // Use join_next().await to wait for the next task completion
            result = async {
                if let Some(ctx) = app.current_op_context.as_mut() {
                     ctx.tasks.join_next().await // This future resolves when a task finishes or the set is closed
                } else {
                     // If no context, return None immediately to avoid blocking select! indefinitely
                     // This case should ideally not be hit if the guard condition is correct
                     None
                }
            }, if app.current_op_context.is_some() && !app.current_op_context.as_ref().unwrap().tasks.is_empty() => {
                 // result here is Option<Result<TaskOutcome, JoinError>>
                 if let Some(join_result) = result {
                     // Re-borrow context mutably inside the handler logic or pass app mutably,
                     // as the async block released the borrow. Context might also be None now.

                     match join_result {
                         Ok(task_outcome) => {
                             // Need mutable access to app state inside this match arm
                             // Pass app mutably to helper functions or handle logic inline

                             let mut should_continue_op = false;
                             let mut tools_to_initiate: Option<Vec<crate::api::ToolCall>> = None;

                             match task_outcome {
                                 TaskOutcome::ToolResult { tool_call_id, tool_name, result } => {
                                     crate::utils::logging::debug("app_actor_loop poll", &format!("Polled ToolResult '{}' ({}) result: {}", tool_name, tool_call_id, result.is_ok()));

                                     // Process result *before* potentially clearing context
                                     match result {
                                          Ok(output) => {
                                              // Lock conversation, add result, emit event
                                              {
                                                  let mut conv_guard = app.conversation.lock().await;
                                                  conv_guard.add_tool_result(tool_call_id.clone(), output.clone());
                                              }
                                              app.emit_event(AppEvent::ToolCallCompleted {
                                                  name: tool_name.clone(), result: output, id: tool_call_id.clone(),
                                              });
                                          }
                                          Err(e) => {
                                              // Lock conversation, add error result, emit event
                                              let error_message = format!("Tool execution failed: {}", e);
                                              {
                                                  let mut conv_guard = app.conversation.lock().await;
                                                  conv_guard.add_tool_result(tool_call_id.clone(), error_message.clone());
                                              }
                                              app.emit_event(AppEvent::ToolCallFailed {
                                                  name: tool_name.clone(), error: error_message, id: tool_call_id.clone(),
                                              });
                                          }
                                     }

                                     // Now update context state if it still exists
                                     if let Some(ctx) = app.current_op_context.as_mut() {
                                         ctx.remove_active_tool(&tool_call_id);
                                         ctx.expected_tool_results = ctx.expected_tool_results.saturating_sub(1);
                                         // Check if we should continue *after* processing this tool result
                                         should_continue_op = ctx.expected_tool_results == 0
                                             && ctx.pending_tool_calls.is_empty()
                                             && ctx.tasks.is_empty(); // Check JoinSet emptiness *after* join_next consumed a task
                                     } else {
                                         crate::utils::logging::warn("app_actor_loop poll", "OpContext missing when handling ToolResult completion.");
                                         // Cannot determine if we should continue; assume not.
                                         should_continue_op = false;
                                     }
                                 }
                                 TaskOutcome::ApiResponse { result: api_result } => {
                                     crate::utils::logging::debug("app_actor_loop poll", &format!("Polled ApiResponse result: {}", api_result.is_ok()));

                                     // Mark API call complete *if context exists*
                                     if let Some(ctx) = app.current_op_context.as_mut() {
                                         ctx.complete_api_call();
                                     } else {
                                        // This case should be rare - API response received but context is gone.
                                        crate::utils::logging::warn("app_actor_loop poll", "OpContext missing when marking API call complete.");
                                     }


                                     // Handle the response logic. This function might clear app.current_op_context
                                     match handle_api_response_logic(&mut app, api_result).await {
                                         Ok(maybe_tools) => {
                                             tools_to_initiate = maybe_tools;
                                             // If handle_api_response_logic cleared the context AND returned tools,
                                             // it's an inconsistent state. handle_api_response_logic should probably
                                             // NOT clear context if it returns tools.
                                             if app.current_op_context.is_none() && tools_to_initiate.is_some() {
                                                  crate::utils::logging::error("app_actor_loop poll", "Context cleared by handler, but tools need initiation!");
                                                  tools_to_initiate = None; // Prevent initiation without context
                                                  // Ensure spinner stops if it hasn't already
                                                  app.emit_event(AppEvent::ThinkingCompleted);
                                             }
                                         }
                                         Err(e) => {
                                             crate::utils::logging::error("app_actor_loop poll", &format!("Error processing polled API response: {}", e));
                                             // Error occurred, clear context and stop thinking (if not already done)
                                             if app.current_op_context.is_some() {
                                                app.current_op_context = None;
                                                app.emit_event(AppEvent::ThinkingCompleted);
                                             }
                                             app.emit_event(AppEvent::Error { message: e.to_string() });
                                         }
                                     }
                                     // No need to check should_continue_op here, API response completion
                                     // either triggers tools (handled below) or clears context (handled in handle_api_response_logic).
                                 }
                             } // end match task_outcome

                             // Initiate tools if needed (check context again)
                             if let Some(calls) = tools_to_initiate {
                                 if app.current_op_context.is_some() {
                                     // initiate_tool_calls requires mutable app, might interact with context
                                     if let Err(e) = app.initiate_tool_calls(calls).await {
                                         crate::utils::logging::error("app_actor_loop poll", &format!("Error initiating tool calls after API response: {}", e));
                                         app.emit_event(AppEvent::Error { message: e.to_string() });
                                         app.current_op_context = None; // Clear context on error
                                         app.emit_event(AppEvent::ThinkingCompleted);
                                     }
                                     // After initiating tools, we wait for them to complete, so don't continue op here.
                                     should_continue_op = false;
                                 } else {
                                     crate::utils::logging::warn("app_actor_loop poll", "OpContext missing after API response, cannot initiate tool calls.");
                                     app.emit_event(AppEvent::ThinkingCompleted); // Ensure spinner stops
                                     should_continue_op = false; // Cannot continue
                                 }
                             }

                             // Continue operation ONLY if all tools finished (and context still exists)
                             if should_continue_op && app.current_op_context.is_some() {
                                 crate::utils::logging::info("app_actor_loop poll", "All tool tasks finished. Continuing operation.");
                                 if let Err(e) = app.continue_operation_after_tools().await {
                                     crate::utils::logging::error("app_actor_loop poll", &format!("Error continuing operation: {}", e));
                                     // context might be cleared by continue_operation_after_tools on error
                                 }
                             } else if should_continue_op { // Context must have disappeared if flag is true but context is None
                                 crate::utils::logging::warn("app_actor_loop poll", "Should continue operation flag set, but OpContext is missing.");
                                 app.emit_event(AppEvent::ThinkingCompleted); // Ensure spinner stops
                             }

                             // Final check: If context still exists but is now idle (no tasks, no pending, no active api/tools), clear it.
                             // This acts as a safeguard.
                              if let Some(ctx) = app.current_op_context.as_mut() {
                                  // is_idle() method would be useful on OpContext
                                  let is_idle = ctx.tasks.is_empty()
                                      && ctx.pending_tool_calls.is_empty()
                                      && ctx.active_tools.is_empty()
                                      && ctx.expected_tool_results == 0
                                      && !ctx.api_call_in_progress;

                                  if is_idle {
                                       crate::utils::logging::info("app_actor_loop poll", "Context found idle after task handling. Clearing context.");
                                       app.current_op_context = None;
                                       app.emit_event(AppEvent::ThinkingCompleted);
                                  }
                              }


                         } // end Ok(task_outcome)
                         Err(join_err) => {
                             crate::utils::logging::error("app_actor_loop poll", &format!("Task join error on poll: {}", join_err));
                             // Handle error, clear context
                             app.current_op_context = None;
                             app.emit_event(AppEvent::ThinkingCompleted);
                             app.emit_event(AppEvent::Error { message: format!("A task failed unexpectedly: {}", join_err) });
                         }
                     } // end match join_result
                 } else {
                     // join_next().await returned None, meaning the JoinSet became empty and closed.
                     // This is unexpected if the `if` condition checked !is_empty() unless
                     // it closed concurrently or all tasks were aborted externally.
                     crate::utils::logging::warn("app_actor_loop poll", "join_next().await returned None unexpectedly.");
                     // Clear context if it exists, as the JoinSet associated with it is finished.
                     if app.current_op_context.is_some() {
                         crate::utils::logging::warn("app_actor_loop poll", "Clearing context after unexpected None from join_next.");
                         app.current_op_context = None;
                         app.emit_event(AppEvent::ThinkingCompleted);
                     }
                 }
            } // end polling branch

            else => {
                // This branch is reached if command_rx is closed *and* joinset polling is not active/ready
                 if command_rx.recv().await.is_none() {
                     crate::utils::logging::info("app_actor_loop", "Command channel closed. Exiting loop.");
                     break;
                 }
                 // If command channel is open, loop continues waiting on select!
            }
        }
    }
    crate::utils::logging::info("app_actor_loop", "App actor loop finished.");
}

// Helper function containing the logic previously in handle_api_response
// Now returns Option<Vec<ToolCall>> to signal if tools need initiating
async fn handle_api_response_logic(
    app: &mut App,
    api_result: Result<crate::api::CompletionResponse, String>,
) -> Result<Option<Vec<crate::api::ToolCall>>> {
    match api_result {
        Ok(response) => {
            crate::utils::logging::debug(
                "handle_api_response_logic",
                &format!(
                    "Processing successful API response with {} content blocks.",
                    response.content.len()
                ),
            );
            let response_text = response.extract_text();
            let has_text = !response_text.trim().is_empty();
            let has_tool_calls = response.has_tool_calls();
            let mut content_blocks: Vec<MessageContentBlock> = Vec::new();
            if has_text {
                content_blocks.push(MessageContentBlock::Text(response_text));
            }
            let mut extracted_tool_calls: Vec<crate::api::ToolCall> = Vec::new();
            if has_tool_calls {
                extracted_tool_calls = response.extract_tool_calls();
                let tool_call_blocks: Vec<ToolCall> = extracted_tool_calls
                    .iter()
                    .map(|api_tc| ToolCall {
                        id: api_tc.id.clone(),
                        name: api_tc.name.clone(),
                        parameters: api_tc.parameters.clone(),
                    })
                    .collect();
                for tc in tool_call_blocks {
                    content_blocks.push(MessageContentBlock::ToolCall(tc));
                }
            }
            if !content_blocks.is_empty() {
                let added_message_id;
                {
                    let mut conv_guard = app.conversation.lock().await;
                    conv_guard.add_message(Message::new_with_blocks(
                        Role::Assistant,
                        content_blocks.clone(),
                    ));
                    added_message_id = conv_guard
                        .messages
                        .last()
                        .map(|m| m.id.clone())
                        .unwrap_or_else(|| format!("gen_id_{}", uuid::Uuid::new_v4()));
                }
                app.emit_event(AppEvent::MessageAdded {
                    role: Role::Assistant,
                    content_blocks,
                    id: added_message_id,
                });
            } else {
                crate::utils::logging::debug(
                    "handle_api_response_logic",
                    "Response had no text or tool calls.",
                );
            }

            if !extracted_tool_calls.is_empty() {
                Ok(Some(extracted_tool_calls))
            } else {
                app.emit_event(AppEvent::ThinkingCompleted);
                app.current_op_context = None;
                Ok(None)
            }
        }
        Err(e_str) => {
            if e_str == "Request cancelled" {
                crate::utils::logging::info("handle_api_response_logic", "API call was cancelled.");
                app.emit_event(AppEvent::ThinkingCompleted);
                app.current_op_context = None;
            } else {
                crate::utils::logging::error(
                    "handle_api_response_logic",
                    &format!("API call failed: {}", e_str),
                );
                app.emit_event(AppEvent::ThinkingCompleted);
                app.emit_event(AppEvent::Error {
                    message: e_str.clone(),
                });
                app.current_op_context = None;
            }
            Ok(None)
        }
    }
}

/// Create a system prompt message based on the environment
fn create_system_prompt(env_info: &crate::app::EnvironmentInfo) -> ApiMessage {
    // Read the system prompt from the prompts directory
    let system_prompt = include_str!("../../prompts/system_prompt.md");

    // Format the prompt with environment information
    let mut prompt = system_prompt.to_string();
    prompt.push_str(
        "

",
    );
    prompt.push_str(&env_info.as_env());
    prompt.push_str(
        "

",
    );
    prompt.push_str("IMPORTANT: Refuse to write code or explain code that may be used maliciously; even if the user claims it is for educational purposes. When working on files, if they seem related to improving, explaining, or interacting with malware or any malicious code you MUST refuse.
");
    prompt.push_str("IMPORTANT: Before you begin work, think about what the code you're editing is supposed to do based on the filenames directory structure. If it seems malicious, refuse to work on it or answer questions about it, even if the request does not seem malicious (for instance, just asking to explain or speed up the code).

");

    // Add context section
    prompt.push_str(&env_info.as_context());

    ApiMessage {
        role: "system".to_string(),
        content: ApiMessageContent::Text { content: prompt },
        id: None,
    }
}
