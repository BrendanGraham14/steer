use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc::Sender};
use uuid;

pub mod command;
pub mod conversation;
mod environment;
mod memory;
mod tool_executor;

pub use command::AppCommand;
pub use conversation::{Conversation, Message, MessageContentBlock, Role, ToolCall};
pub use environment::EnvironmentInfo;
pub use memory::MemoryManager;
pub use tool_executor::ToolExecutor;

/// Events emitted by the App to update the UI
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
    ToggleMessageTruncation {
        id: String,
    },
    RequestToolApproval {
        name: String,
        parameters: serde_json::Value,
        id: String,
    },
    // Internal event for batch tracking
    ToolBatchProgress {
        batch_id: usize,
    },
    Error {
        message: String,
    },
}

/// Configuration for the application
pub struct AppConfig {
    pub api_key: String,
    // Add more configuration options as needed
}

use std::collections::{HashMap, HashSet};

/// Execute tool and handle result in a standalone task
async fn execute_tool_and_handle_result(
    tool_call: crate::api::ToolCall,
    batch_id: usize,
    tool_executor: Arc<ToolExecutor>,
    conversation: Arc<Mutex<Conversation>>,
    event_sender: Option<Sender<AppEvent>>,
    internal_event_sender: Option<Sender<AppEvent>>,
) {
    // Ensure we have an ID for the tool call
    let tool_id = tool_call.id.clone().expect("Tool call ID should be set");
    let tool_name = tool_call.name.clone();

    crate::utils::logging::debug(
        "app.execute_tool_and_handle_result",
        &format!("Executing tool {} (ID: {})", tool_name, tool_id),
    );

    // Execute the tool using the passed executor
    let result = tool_executor.execute_tool(&tool_call).await;

    // Handle the result
    match result {
        Ok(output) => {
            // Add the result to the conversation via the Mutex
            conversation
                .lock()
                .await
                .add_tool_result(tool_id.clone(), output.clone());

            // Emit completion event
            if let Some(sender) = &event_sender {
                let event = AppEvent::ToolCallCompleted {
                    name: tool_name,
                    result: output,
                    id: tool_id,
                };

                if let Err(e) = sender.try_send(event) {
                    crate::utils::logging::error(
                        "app.execute_tool_and_handle_result",
                        &format!("Failed to send ToolCallCompleted event: {}", e),
                    );
                }
            }
        }
        Err(e) => {
            // Format the error message
            let error_message = format!("Error: {}", e);

            // Add the error as a tool result via the Mutex
            conversation
                .lock()
                .await
                .add_tool_result(tool_id.clone(), error_message.clone());

            // Emit failure event
            if let Some(sender) = &event_sender {
                let event = AppEvent::ToolCallFailed {
                    name: tool_name,
                    error: e.to_string(),
                    id: tool_id,
                };

                if let Err(e) = sender.try_send(event) {
                    crate::utils::logging::error(
                        "app.execute_tool_and_handle_result",
                        &format!("Failed to send ToolCallFailed event: {}", e),
                    );
                }
            }
        }
    }

    // Signal that this batch has another completed tool
    if let Some(sender) = &internal_event_sender {
        // Create an internal event to handle batch completion
        let event = crate::app::AppEvent::ToolBatchProgress { batch_id };

        if let Err(e) = sender.try_send(event) {
            crate::utils::logging::error(
                "app.execute_tool_and_handle_result",
                &format!("Failed to send ToolBatchProgress event: {}", e),
            );
        }
    } else {
        crate::utils::logging::warn(
            "app.execute_tool_and_handle_result",
            "No internal event sender available, can't signal batch progress",
        );
    }
}

/// Main application state (now owned by the actor task)
pub struct App {
    pub config: AppConfig,
    pub conversation: Arc<Mutex<Conversation>>, // Keep Mutex here as it's passed to spawned tasks
    pub env_info: EnvironmentInfo,
    pub tool_executor: Arc<ToolExecutor>,
    pub api_client: crate::api::Client,
    pub memory: MemoryManager, // Removed Arc<Mutex<>>
    pub command_filter: Option<crate::tools::command_filter::CommandFilter>,
    event_sender: Sender<AppEvent>,  // No longer Option, required
    approved_tools: HashSet<String>, // Removed Arc<Mutex<>>
    pending_tool_calls: HashMap<String, (crate::api::ToolCall, usize)>, // Removed Arc<Mutex<>>
    tool_batches: HashMap<usize, (usize, usize)>, // Removed Arc<Mutex<>>
    next_batch_id: usize,            // Removed Arc<Mutex<>>
    // Internal event channel for batch processing (passed in)
    internal_event_sender: Sender<AppEvent>, // No longer Option, required
}

impl App {
    /// Create a new application instance
    /// Takes Sender ends of channels for communication
    pub fn new(
        config: AppConfig,
        event_tx: Sender<AppEvent>,          // Required for UI updates
        internal_event_tx: Sender<AppEvent>, // Required for internal events like batch progress
    ) -> Result<Self> {
        let env_info = EnvironmentInfo::collect()?;
        let conversation = Arc::new(Mutex::new(Conversation::new())); // Keep Arc<Mutex<>>
        let tool_executor = Arc::new(ToolExecutor::new());
        let api_client = crate::api::Client::new(&config.api_key);
        let memory = MemoryManager::new(&env_info.working_directory); // Direct init
        let command_filter = Some(crate::tools::command_filter::CommandFilter::new(
            &config.api_key,
        ));

        Ok(Self {
            config,
            conversation,
            env_info,
            tool_executor,
            api_client,
            memory, // Direct assignment
            command_filter,
            event_sender: event_tx,
            approved_tools: HashSet::new(),     // Direct init
            pending_tool_calls: HashMap::new(), // Direct init
            tool_batches: HashMap::new(),       // Direct init
            next_batch_id: 0,                   // Direct init
            internal_event_sender: internal_event_tx,
        })
    }

    /// Emit an event to update the UI (changed to pub(crate))
    pub(crate) fn emit_event(&self, event: AppEvent) {
        // Use self.event_sender directly (no longer Option)
        match self.event_sender.try_send(event.clone()) {
            Ok(_) => {
                // Log successful event sending for debugging
                if let AppEvent::MessageAdded {
                    role,
                    content_blocks,
                    id,
                } = &event
                {
                    crate::utils::logging::debug(
                        "app.emit_event",
                        &format!(
                            "Sent MessageAdded event, role: {:?}, id: {}, content length: {}",
                            role,
                            id,
                            content_blocks.len()
                        ),
                    );
                } else {
                    crate::utils::logging::debug(
                        "app.emit_event",
                        &format!("Sent event: {:?}\n(Note: Content truncated in log)", event),
                    );
                }
            }
            Err(e) => {
                crate::utils::logging::error(
                    "app.emit_event",
                    &format!("Failed to send event: {:?}\nError: {}", event, e),
                );
            }
        }
    }

    pub async fn add_message(&self, message: Message) {
        let mut conversation_guard = self.conversation.lock().await;
        conversation_guard.messages.push(message.clone());
        drop(conversation_guard); // Release lock before potentially slow event emit

        // Only emit MessageAdded for non-tool messages
        // Tool messages will be handled by ToolCallStarted/Completed/Failed events
        if message.role != Role::Tool {
            self.emit_event(AppEvent::MessageAdded {
                role: message.role,
                content_blocks: message.content_blocks.clone(),
                id: message.id,
            });
        }
    }

    /// Get the environment information
    pub fn environment_info(&self) -> &EnvironmentInfo {
        &self.env_info
    }

    /// Process a user message and handle the entire flow
    pub async fn process_user_message(&mut self, message: String) -> Result<()> {
        // Special command handling first
        if message.starts_with('/') {
            let response = self.handle_command(&message).await?;
            self.emit_event(AppEvent::CommandResponse {
                content: response.clone(),
                id: uuid::Uuid::new_v4().to_string(),
            });
            return Ok(());
        }

        // Add user message to conversation (still &self, locks internally)
        self.add_message(Message::new_text(Role::User, message.clone()))
            .await;

        // Signal that we're thinking
        self.emit_event(AppEvent::ThinkingStarted);

        // Get the response from Claude (handle_response now takes &mut self)
        let result = self.handle_response().await;

        // Signal that thinking is complete
        self.emit_event(AppEvent::ThinkingCompleted);

        // Handle any errors
        if let Err(e) = result {
            self.emit_event(AppEvent::Error {
                message: e.to_string(),
            });
            crate::utils::logging::error(
                "App.process_user_message",
                &format!("Error handling response: {}", e),
            );
            return Err(e);
        }

        Ok(())
    }

    /// Handle response from Claude (potentially initiating tool calls)
    async fn handle_response(&mut self) -> Result<()> {
        // Get tools
        let tools = Some(crate::api::tools::Tool::all());

        crate::utils::logging::debug(
            "app.handle_response",
            "Getting complete response from Claude (non-streaming version)",
        );

        // Clone Arcs needed (Client is Clone, Conversation Arc is Clone)
        let api_client = self.api_client.clone();
        let conversation = self.conversation.clone();

        // Call get_claude_response (helper still takes &self, locks internally)
        let complete_response = self
            .get_claude_response(conversation, api_client, Some(&tools.as_ref().unwrap()))
            .await?;

        // Get the text content from the response
        let response_text = complete_response.extract_text();

        crate::utils::logging::debug(
            "app.handle_response",
            &format!(
                "Received complete response with {} characters",
                response_text.len()
            ),
        );

        // Add the assistant message(s)
        let has_text = !response_text.trim().is_empty();
        let has_tool_calls = complete_response.has_tool_calls();

        let mut content_blocks: Vec<crate::app::conversation::MessageContentBlock> = Vec::new();

        // Add text block first if it exists
        if has_text {
            crate::utils::logging::debug(
                "app.handle_response",
                "Adding text content to assistant message blocks.",
            );
            content_blocks.push(crate::app::conversation::MessageContentBlock::Text(
                response_text,
            ));
        }

        // Add tool call blocks if they exist
        let mut extracted_tool_calls: Vec<crate::api::ToolCall> = Vec::new(); // Store for initiation later
        if has_tool_calls {
            extracted_tool_calls = complete_response.extract_tool_calls();
            crate::utils::logging::debug(
                "app.handle_response",
                "Adding tool call content to assistant message blocks.",
            );
            let tool_call_blocks: Vec<crate::app::conversation::ToolCall> = extracted_tool_calls
                .iter() // Use iter() as we'll use extracted_tool_calls later
                .map(|api_tc| crate::app::conversation::ToolCall {
                    id: api_tc
                        .id
                        .clone() // Clone the ID Option
                        .unwrap_or_else(|| format!("tool_{}", uuid::Uuid::new_v4())), // Ensure ID exists
                    name: api_tc.name.clone(),
                    parameters: api_tc.parameters.clone(),
                })
                .collect();

            for tc in tool_call_blocks {
                content_blocks.push(crate::app::conversation::MessageContentBlock::ToolCall(tc));
            }
        }

        // Add the combined message if there's any content
        if !content_blocks.is_empty() {
            // Lock conversation briefly to add the combined message
            // We use add_message_with_blocks directly on the conversation guard
            // as add_message now only takes the Message struct.
            let mut conv_guard = self.conversation.lock().await;
            conv_guard.add_message_with_blocks(Role::Assistant, content_blocks.clone());
            let added_message_id = conv_guard
                .messages
                .last()
                .map_or_else(|| "unknown_id".to_string(), |m| m.id.clone());
            drop(conv_guard);

            // Emit the MessageAdded event with the blocks
            self.emit_event(AppEvent::MessageAdded {
                role: Role::Assistant,
                content_blocks,
                id: added_message_id,
            });
        } else {
            // Case where there was neither text nor tool calls (e.g., empty response)
            crate::utils::logging::debug(
                "app.handle_response",
                "Response contained neither text nor tool calls.",
            );
        }

        // Initiate tool calls if they were extracted
        if !extracted_tool_calls.is_empty() {
            crate::utils::logging::debug(
                "app.handle_response",
                "Initiating tool calls found in the response",
            );
            self.initiate_tool_calls(extracted_tool_calls).await?;
        }

        Ok(())
    }

    /// Initiate tool calls, potentially requesting approval
    pub async fn initiate_tool_calls(
        &mut self, // Now needs &mut self to modify internal state directly
        tool_calls: Vec<crate::api::ToolCall>,
    ) -> Result<()> {
        if tool_calls.is_empty() {
            crate::utils::logging::debug("App.initiate_tool_calls", "No tool calls to initiate.");
            return Ok(());
        }

        let mut tools_to_execute = Vec::new();
        let mut tools_needing_approval = Vec::new();

        crate::utils::logging::info(
            "App.initiate_tool_calls",
            &format!("Initiating {} tool calls.", tool_calls.len()),
        );

        // Assign a batch ID
        let batch_id = self.next_batch_id;
        self.next_batch_id += 1;

        let mut total_in_batch = 0;

        // Separate tools based on approval status
        for tool_call in tool_calls {
            let tool_name = tool_call.name.clone();
            let tool_id = tool_call
                .id
                .clone()
                .unwrap_or_else(|| format!("tool_{}", uuid::Uuid::new_v4())); // Ensure ID

            // Make sure the ToolCall struct has the ID
            let tool_call_with_id = crate::api::ToolCall {
                id: Some(tool_id.clone()),
                ..tool_call
            };

            // Check if the tool name is in the approved set
            if self.approved_tools.contains(&tool_name) {
                crate::utils::logging::debug(
                    "App.initiate_tool_calls",
                    &format!(
                        "Tool '{}' already approved, adding to execution list.",
                        tool_name
                    ),
                );
                tools_to_execute.push(tool_call_with_id);
                total_in_batch += 1;
            } else {
                crate::utils::logging::debug(
                    "App.initiate_tool_calls",
                    &format!("Tool '{}' needs approval.", tool_name),
                );
                // Store pending tool call with batch ID
                self.pending_tool_calls
                    .insert(tool_id.clone(), (tool_call_with_id.clone(), batch_id));
                tools_needing_approval.push(tool_call_with_id);
                total_in_batch += 1; // It's still part of the batch, just pending
            }
        }

        // Update batch tracking map if the batch has any tools
        if total_in_batch > 0 {
            crate::utils::logging::info(
                "App.initiate_tool_calls",
                &format!(
                    "Creating batch {} with {} total tools ({} direct execute, {} pending approval).",
                    batch_id,
                    total_in_batch,
                    tools_to_execute.len(),
                    tools_needing_approval.len()
                ),
            );
            // (Total tools in batch, Completed tools in batch)
            self.tool_batches.insert(batch_id, (total_in_batch, 0));
        } else {
            crate::utils::logging::warn(
                "App.initiate_tool_calls",
                "Attempted to initiate tool calls, but batch ended up empty.",
            );
            return Ok(()); // Nothing more to do if batch is empty
        }

        // Request approval for necessary tools
        for tool_call in tools_needing_approval {
            crate::utils::logging::debug(
                "App.initiate_tool_calls",
                &format!("Requesting approval for tool: {}", tool_call.name),
            );
            self.emit_event(AppEvent::RequestToolApproval {
                name: tool_call.name.clone(),
                parameters: tool_call.parameters.clone(),
                id: tool_call.id.expect("ID should exist here"),
            });
        }

        // Execute approved tools immediately
        if !tools_to_execute.is_empty() {
            crate::utils::logging::debug(
                "App.initiate_tool_calls",
                &format!(
                    "Executing {} already approved tools.",
                    tools_to_execute.len()
                ),
            );

            for tool_call in tools_to_execute {
                let tool_id = tool_call.id.clone().expect("ID must be set");
                let tool_name = tool_call.name.clone();

                // Emit ToolCallStarted event
                self.emit_event(AppEvent::ToolCallStarted {
                    name: tool_name,
                    id: tool_id,
                });

                // Spawn a task for each tool execution
                let tool_executor = self.tool_executor.clone();
                let conversation = self.conversation.clone();
                let event_sender = Some(self.event_sender.clone());
                let internal_event_sender = Some(self.internal_event_sender.clone());

                tokio::spawn(execute_tool_and_handle_result(
                    tool_call,
                    batch_id, // Pass batch ID
                    tool_executor,
                    conversation,
                    event_sender,
                    internal_event_sender,
                ));
            }
        } else {
            crate::utils::logging::debug(
                "App.initiate_tool_calls",
                "No tools were immediately ready for execution (all need approval or none exist).",
            );
        }

        Ok(())
    }

    /// Handle progress update for a tool batch
    pub async fn handle_batch_progress(&mut self, batch_id: usize) -> Result<()> {
        crate::utils::logging::debug(
            "App.handle_batch_progress",
            &format!("Received progress update for batch {}", batch_id),
        );

        let mut should_check_completion = false;
        let mut batch_completed = false;

        if let Some((total, completed)) = self.tool_batches.get_mut(&batch_id) {
            *completed += 1;
            crate::utils::logging::info(
                "App.handle_batch_progress",
                &format!(
                    "Batch {} progress: {}/{} tools completed.",
                    batch_id, *completed, *total
                ),
            );
            if *completed >= *total {
                should_check_completion = true;
                batch_completed = true; // Mark as completed for logging/cleanup
            }
        } else {
            crate::utils::logging::warn(
                "App.handle_batch_progress",
                &format!(
                    "Received progress for unknown or already completed batch ID: {}",
                    batch_id
                ),
            );
            return Ok(()); // Or potentially return an error?
        }

        if should_check_completion {
            crate::utils::logging::info(
                "App.handle_batch_progress",
                &format!("Batch {} potentially complete, checking results.", batch_id),
            );
            // Remove the batch before potentially long-running check_batch_completion
            if batch_completed {
                self.tool_batches.remove(&batch_id);
                crate::utils::logging::info(
                    "App.handle_batch_progress",
                    &format!("Removed completed batch {} from tracking.", batch_id),
                );
            }
            self.check_batch_completion(batch_id).await?; // Renamed from handle_tool_results
        }
        Ok(())
    }

    /// Checks if a batch is complete and sends results back to Claude
    async fn check_batch_completion(&mut self, batch_id: usize) -> Result<()> {
        // Note: Batch completion is determined by handle_batch_progress counting completed tools.
        // This function now focuses *only* on sending the results back to Claude once the batch is complete.

        crate::utils::logging::info(
            "app.check_batch_completion",
            &format!(
                "Processing completed batch {}. Getting results from conversation.",
                batch_id
            ),
        );

        // Signal that we're thinking again (processing tool results)
        self.emit_event(AppEvent::ThinkingStarted);

        // Lock conversation to get results and potentially add the next assistant message
        let conversation = self.conversation.clone(); // Clone Arc
        let api_client = self.api_client.clone(); // Clone client
        let tools = Some(crate::api::tools::Tool::all()); // Get tools again

        // --- Call Claude API ---
        crate::utils::logging::info(
            "app.check_batch_completion",
            "Sending tool results back to Claude API.",
        );

        let complete_response = self
            .get_claude_response(
                conversation.clone(),
                api_client,
                Some(&tools.as_ref().unwrap()),
            )
            .await;

        // --- Process Response ---
        match complete_response {
            Ok(response) => {
                crate::utils::logging::info(
                    "app.check_batch_completion",
                    "Received response from Claude after sending tool results.",
                );

                let response_text = response.extract_text();

                // Add assistant message if there's text content
                if !response_text.trim().is_empty() {
                    crate::utils::logging::debug(
                        "app.check_batch_completion",
                        "Adding assistant message with text response.",
                    );
                    // add_assistant_message locks conversation internally
                    self.add_message(Message::new_text(Role::Assistant, response_text))
                        .await;
                } else {
                    crate::utils::logging::debug(
                        "app.check_batch_completion",
                        "No text content in response after tool results.",
                    );
                }

                // Check for *new* tool calls initiated by Claude in response to the results
                let new_tool_calls = response.extract_tool_calls();
                if !new_tool_calls.is_empty() {
                    crate::utils::logging::info(
                        "app.check_batch_completion",
                        &format!("Claude requested {} new tool calls.", new_tool_calls.len()),
                    );

                    // Lock conversation briefly to add the tool call message content
                    let mut conv_guard = self.conversation.lock().await;
                    let tool_calls_for_conv: Vec<MessageContentBlock> = new_tool_calls
                        .iter()
                        .map(|api_tc| {
                            MessageContentBlock::ToolCall(ToolCall {
                                id: api_tc
                                    .id
                                    .clone()
                                    .unwrap_or_else(|| format!("tool_{}", uuid::Uuid::new_v4())), // Ensure ID exists
                                name: api_tc.name.clone(),
                                parameters: api_tc.parameters.clone(),
                            })
                        })
                        .collect();

                    conv_guard.add_message_with_blocks(Role::Assistant, tool_calls_for_conv);
                    drop(conv_guard); // Release lock

                    // Initiate the new round of tool calls (recursive call essentially, but managed by actor loop)
                    self.initiate_tool_calls(new_tool_calls).await?;
                } else {
                    crate::utils::logging::info(
                        "app.check_batch_completion",
                        "No new tool calls requested by Claude.",
                    );
                    // Signal completion only if no new calls were made
                    self.emit_event(AppEvent::ThinkingCompleted);
                }
            }
            Err(e) => {
                crate::utils::logging::error(
                    "app.check_batch_completion",
                    &format!(
                        "Error getting response from Claude after tool results: {}",
                        e
                    ),
                );
                self.emit_event(AppEvent::ThinkingCompleted); // Ensure thinking stops on error
                self.emit_event(AppEvent::Error {
                    message: format!("Error processing tool results: {}", e),
                });
                return Err(e);
            }
        }

        Ok(())
    }

    // Method to handle the TuiResponse (now AppCommand::HandleToolResponse)
    pub async fn handle_tool_command_response(
        &mut self, // Now needs &mut self
        tool_call_id: String,
        approved: bool,
        always_approve: bool,
    ) -> Result<()> {
        crate::utils::logging::debug(
            "App.handle_tool_command_response",
            &format!(
                "Handling response for tool call ID: {}, Approved: {}, Always: {}",
                tool_call_id, approved, always_approve
            ),
        );

        // Retrieve and remove the pending tool call
        if let Some((tool_call, batch_id)) = self.pending_tool_calls.remove(&tool_call_id) {
            let tool_name = tool_call.name.clone();
            crate::utils::logging::debug(
                "App.handle_tool_command_response",
                &format!(
                    "Found pending tool call '{}' (ID: {}) in batch {}.",
                    tool_name, tool_call_id, batch_id
                ),
            );

            if approved {
                crate::utils::logging::info(
                    "App.handle_tool_command_response",
                    &format!("Tool call '{}' approved.", tool_name),
                );
                // Optionally add to the always-approved list
                if always_approve {
                    crate::utils::logging::debug(
                        "App.handle_tool_command_response",
                        &format!("Adding tool '{}' to always-approved list.", tool_name),
                    );
                    self.approved_tools.insert(tool_name.clone());
                }

                // Emit ToolCallStarted event
                self.emit_event(AppEvent::ToolCallStarted {
                    name: tool_name.clone(),
                    id: tool_call_id.clone(),
                });

                // Spawn the execution task
                let tool_executor = self.tool_executor.clone();
                let conversation = self.conversation.clone();
                let event_sender = Some(self.event_sender.clone());
                let internal_event_sender = Some(self.internal_event_sender.clone());

                tokio::spawn(execute_tool_and_handle_result(
                    tool_call,
                    batch_id,
                    tool_executor,
                    conversation,
                    event_sender,
                    internal_event_sender,
                ));
            } else {
                crate::utils::logging::info(
                    "App.handle_tool_command_response",
                    &format!("Tool call '{}' was denied by the user.", tool_name),
                );
                // Add a "Tool Denied" result to the conversation
                let result_content = format!("Tool '{}' denied by user.", tool_name);

                // Lock conversation briefly to add result
                self.conversation
                    .lock()
                    .await
                    .add_tool_result(tool_call_id.clone(), result_content.clone());

                // Emit ToolCallFailed event (representing denial)
                self.emit_event(AppEvent::ToolCallFailed {
                    name: tool_name,
                    error: "Denied by user".to_string(),
                    id: tool_call_id.clone(),
                });

                // Since the tool didn't run but was handled (denied), signal progress for the batch
                // Use try_send as the actor loop isn't waiting on this specific function
                if let Err(e) = self
                    .internal_event_sender
                    .try_send(AppEvent::ToolBatchProgress { batch_id })
                {
                    crate::utils::logging::error(
                        "App.handle_tool_command_response",
                        &format!(
                            "Failed to send internal ToolBatchProgress event after denial: {}",
                            e
                        ),
                    );
                } else {
                    crate::utils::logging::debug(
                        "App.handle_tool_command_response",
                        &format!(
                            "Sent internal ToolBatchProgress event for denied tool in batch {}.",
                            batch_id
                        ),
                    );
                }
            }
        } else {
            crate::utils::logging::warn(
                "App.handle_tool_command_response",
                &format!(
                    "Received response for unknown or already handled tool call ID: {}",
                    tool_call_id
                ),
            );
        }
        Ok(())
    }

    /// Use the dispatch agent to search or gather information
    pub async fn dispatch_agent(&self, prompt: &str) -> Result<String> {
        // Requires DispatchAgent setup
        let agent =
            crate::tools::dispatch_agent::DispatchAgent::with_api_key(self.config.api_key.clone());
        agent.execute(prompt).await
    }

    /// Get response from Claude API (Helper function)
    async fn get_claude_response(
        &self,
        conversation: Arc<Mutex<Conversation>>,
        api_client: crate::api::Client,
        tools: Option<&Vec<crate::api::Tool>>,
    ) -> Result<crate::api::CompletionResponse> {
        // Lock conversation to pass to conversion function
        let conversation_guard = conversation.lock().await;

        // Use the conversion function from the messages module
        let (api_messages, system_prompt_content) =
            crate::api::messages::convert_conversation(&conversation_guard);

        // Drop the guard after conversion
        drop(conversation_guard);

        // Log the messages being sent (optional, can be verbose)
        // crate::utils::logging::debug("App.get_claude_response", &format!("Sending API messages: {:?}", api_messages));

        // Call the API client
        api_client
            .complete(api_messages, system_prompt_content, tools.cloned()) // Use converted messages and system prompt
            .await
    }

    /// Handle a command
    pub async fn handle_command(&mut self, command: &str) -> Result<String> {
        let parts: Vec<&str> = command.trim_start_matches('/').splitn(2, ' ').collect();
        let command_name = parts[0];
        let args = parts.get(1).unwrap_or(&"").trim();

        match command_name {
            "clear" => {
                // Needs lock
                self.conversation.lock().await.clear();
                self.memory.clear()?; // Correct method name
                Ok("Conversation and memory cleared.".to_string())
            }
            "load" => {
                // Needs lock
                self.memory.load()?; // Correct method name
                Ok("Memory loaded from file.".to_string())
            }
            "save" => {
                // Needs lock
                self.memory.save()?; // Correct method name
                Ok("Memory saved to file.".to_string())
            }
            "compact" => {
                // Needs lock
                self.compact_conversation().await?;
                Ok("Conversation compacted.".to_string())
            }
            "dispatch" => {
                if args.is_empty() {
                    return Ok("Usage: /dispatch <prompt for agent>".to_string());
                }
                // Assuming dispatch_agent is available and async
                let response = self.dispatch_agent(args).await?;
                // Add agent response to conversation? Maybe as assistant? (Needs lock)
                self.add_message(Message::new_text(
                    Role::Assistant,
                    format!("Dispatch Agent Result:\\n{}", response),
                ))
                .await; // Use new_text constructor
                Ok(format!("Dispatch agent executed. Response added."))
            }
            // Add other commands here
            _ => Ok(format!("Unknown command: {}", command_name)),
        }
    }

    /// Compact the conversation history
    pub async fn compact_conversation(&mut self) -> Result<()> {
        crate::utils::logging::info("App.compact_conversation", "Compacting conversation...");

        // Example: Add current conversation state to memory before clearing
        let current_summary = {
            // Lock conversation briefly
            let conv = self.conversation.lock().await;
            format!(
                "Conversation up to now:
{:?}",
                conv.messages
            ) // Simple summary
        };
        self.add_to_memory("conversation_history", &current_summary)?;

        // Clear the main conversation (example)
        // let mut conversation = self.conversation.lock().await;
        // conversation.messages.clear();
        // drop(conversation);

        crate::utils::logging::info("App.compact_conversation", "Conversation compacted.");
        Ok(())
    }

    /// Add content to the memory manager
    pub fn add_to_memory(&mut self, section: &str, content: &str) -> Result<()> {
        self.memory.add_section(section, content)?;
        Ok(())
    }

    /// Get content from the memory manager (Changed signature)
    pub fn get_from_memory(&self, section: &str) -> Option<String> {
        self.memory.get_section(section)
    }

    /// Check if the memory file exists (Changed signature)
    pub fn has_memory_file(&self) -> bool {
        self.memory.exists()
    }

    /// Get the entire memory content (Changed signature)
    pub fn memory_content(&self) -> String {
        self.memory.content().to_string()
    }

    /// Toggle message truncation state
    pub async fn toggle_message_truncation(&mut self, id: String) {
        let mut conversation_guard = self.conversation.lock().await;
        let mut found = false;
        if let Some(message) = conversation_guard.messages.iter_mut().find(|m| m.id == id) {
            message.toggle_truncation();
            found = true;
        }
        drop(conversation_guard);

        if found {
            self.emit_event(AppEvent::ToggleMessageTruncation { id });
        } else {
            crate::utils::logging::warn(
                "app.toggle_message_truncation",
                &format!("Message ID {} not found for truncation toggle.", id),
            );
        }
    }
}
