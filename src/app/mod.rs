use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify, mpsc::Sender};
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
    ToolBatchProgress {
        batch_id: usize,
        tool_call_id: String,
    },
    Error {
        message: String,
    },
}

pub struct AppConfig {
    pub api_key: String,
}

use std::collections::{HashMap, HashSet};

async fn execute_tool_and_handle_result(
    tool_call: crate::api::ToolCall,
    batch_id: usize,
    tool_executor: Arc<ToolExecutor>,
    conversation: Arc<Mutex<Conversation>>,
    event_sender: Option<Sender<AppEvent>>,
    internal_event_sender: Option<Sender<AppEvent>>,
) {
    let tool_id = tool_call.id.clone();
    let tool_name = tool_call.name.clone();

    crate::utils::logging::debug(
        "app.execute_tool_and_handle_result",
        &format!("Executing tool {} (ID: {})", tool_name, tool_id),
    );

    let result = tool_executor.execute_tool(&tool_call).await;

    match result {
        Ok(output) => {
            conversation
                .lock()
                .await
                .add_tool_result(tool_id.clone(), output.clone());

            if let Some(sender) = &event_sender {
                let event = AppEvent::ToolCallCompleted {
                    name: tool_name,
                    result: output,
                    id: tool_id.clone(),
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
            let error_message = format!("Error: {}", e);

            conversation
                .lock()
                .await
                .add_tool_result(tool_id.clone(), error_message.clone());

            if let Some(sender) = &event_sender {
                let event = AppEvent::ToolCallFailed {
                    name: tool_name,
                    error: e.to_string(),
                    id: tool_id.clone(),
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

    if let Some(sender) = &internal_event_sender {
        let event = crate::app::AppEvent::ToolBatchProgress {
            batch_id,
            tool_call_id: tool_id,
        };

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

pub struct App {
    pub config: AppConfig,
    pub conversation: Arc<Mutex<Conversation>>,
    pub env_info: EnvironmentInfo,
    pub tool_executor: Arc<ToolExecutor>,
    pub api_client: crate::api::Client,
    pub memory: MemoryManager,
    pub command_filter: Option<crate::tools::command_filter::CommandFilter>,
    event_sender: Sender<AppEvent>,
    approved_tools: HashSet<String>,
    pending_tool_calls: HashMap<String, (crate::api::ToolCall, usize)>,
    tool_batches: HashMap<usize, (usize, usize)>,
    next_batch_id: usize,
    internal_event_sender: Sender<AppEvent>,
    active_tool_tasks: HashMap<String, (tokio::task::JoinHandle<()>, usize)>,
    cancellation_notifier: Arc<Notify>,
}

impl App {
    pub fn new(
        config: AppConfig,
        event_tx: Sender<AppEvent>,
        internal_event_tx: Sender<AppEvent>,
    ) -> Result<Self> {
        let env_info = EnvironmentInfo::collect()?;
        let conversation = Arc::new(Mutex::new(Conversation::new()));
        let tool_executor = Arc::new(ToolExecutor::new());
        let api_client = crate::api::Client::new(&config.api_key);
        let memory = MemoryManager::new(&env_info.working_directory);
        let command_filter = Some(crate::tools::command_filter::CommandFilter::new(
            &config.api_key,
        ));

        Ok(Self {
            config,
            conversation,
            env_info,
            tool_executor,
            api_client,
            memory,
            command_filter,
            event_sender: event_tx,
            approved_tools: HashSet::new(),
            pending_tool_calls: HashMap::new(),
            tool_batches: HashMap::new(),
            next_batch_id: 0,
            internal_event_sender: internal_event_tx,
            active_tool_tasks: HashMap::new(),
            cancellation_notifier: Arc::new(Notify::new()),
        })
    }

    pub(crate) fn emit_event(&self, event: AppEvent) {
        match self.event_sender.try_send(event.clone()) {
            Ok(_) => {
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
        drop(conversation_guard);

        if message.role != Role::Tool {
            self.emit_event(AppEvent::MessageAdded {
                role: message.role,
                content_blocks: message.content_blocks.clone(),
                id: message.id,
            });
        }
    }

    pub fn environment_info(&self) -> &EnvironmentInfo {
        &self.env_info
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

        // Interrupt any running tools first
        self.cancel_current_processing();

        // Add user message
        self.add_message(Message::new_text(Role::User, message.clone()))
            .await;

        // Start thinking and call handle_response
        self.emit_event(AppEvent::ThinkingStarted);

        // handle_response now internally handles cancellation
        if let Err(e) = self.handle_response().await {
            crate::utils::logging::error(
                "App.process_user_message",
                &format!("Error during handle_response: {}", e),
            );
            // Ensure ThinkingCompleted is emitted even if handle_response errors early
            // (handle_response should emit it on success or cancellation)
            self.emit_event(AppEvent::ThinkingCompleted);
            self.emit_event(AppEvent::Error {
                message: e.to_string(),
            });
            return Err(e);
        }

        Ok(())
    }

    async fn handle_response(&mut self) -> Result<()> {
        let tools = Some(crate::api::tools::Tool::all());

        crate::utils::logging::debug(
            "app.handle_response",
            "Getting Claude response (cancellable)...",
        );

        let api_client = self.api_client.clone();
        let conversation = self.conversation.clone();
        let notifier = self.cancellation_notifier.clone(); // Clone Arc<Notify>

        tokio::select! {
            biased; // Check notification first

            _ = notifier.notified() => {
                crate::utils::logging::info("App.handle_response", "API call cancelled via notification.");
                 // ThinkingCompleted should have been emitted by cancel_current_processing
                Ok(())
            }

            response_result = self.get_claude_response(conversation, api_client, tools.as_ref()) => {
                match response_result {
                    Ok(response) => {
                        crate::utils::logging::debug(
                            "App.handle_response",
                            &format!("Received API response with {} content blocks.", response.content.len()),
                        );
                        // Process the successful response
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
                            let tool_call_blocks: Vec<ToolCall> = extracted_tool_calls.iter()
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
                            let mut conv_guard = self.conversation.lock().await;
                            conv_guard.add_message_with_blocks(Role::Assistant, content_blocks.clone());
                            let added_message_id = conv_guard.messages.last().map_or_else(|| "unknown_id".to_string(), |m| m.id.clone());
                            drop(conv_guard);
                            self.emit_event(AppEvent::MessageAdded {
                                role: Role::Assistant,
                                content_blocks,
                                id: added_message_id,
                            });
                        } else {
                            crate::utils::logging::debug("App.handle_response", "Response had no text or tool calls.");
                        }

                        if !extracted_tool_calls.is_empty() {
                            self.initiate_tool_calls(extracted_tool_calls).await?;
                            // ThinkingCompleted will be handled by tool completion/error
                        } else {
                            self.emit_event(AppEvent::ThinkingCompleted); // No tools, thinking done
                        }
                        Ok(())
                    }
                    Err(e) => {
                        crate::utils::logging::error("App.handle_response", &format!("API call failed: {}", e));
                        self.emit_event(AppEvent::ThinkingCompleted); // Stop spinner on error
                        self.emit_event(AppEvent::Error { message: e.to_string() });
                        Err(e) // Propagate the error
                    }
                }
            }
        }
    }

    pub async fn initiate_tool_calls(
        &mut self,
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

        let batch_id = self.next_batch_id;
        self.next_batch_id += 1;

        let mut total_in_batch = 0;

        for tool_call in tool_calls {
            let tool_name = tool_call.name.clone();

            let tool_call_with_id = crate::api::ToolCall {
                id: tool_call.id.clone(),
                ..tool_call
            };

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
                self.pending_tool_calls
                    .insert(tool_call.id.clone(), (tool_call_with_id.clone(), batch_id));
                tools_needing_approval.push(tool_call_with_id);
                total_in_batch += 1;
            }
        }

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
            self.tool_batches.insert(batch_id, (total_in_batch, 0));
        } else {
            crate::utils::logging::warn(
                "App.initiate_tool_calls",
                "Attempted to initiate tool calls, but batch ended up empty.",
            );
            return Ok(());
        }

        for tool_call in tools_needing_approval {
            crate::utils::logging::debug(
                "App.initiate_tool_calls",
                &format!("Requesting approval for tool: {}", tool_call.name),
            );
            self.emit_event(AppEvent::RequestToolApproval {
                name: tool_call.name.clone(),
                parameters: tool_call.parameters.clone(),
                id: tool_call.id.clone(),
            });
        }

        if !tools_to_execute.is_empty() {
            crate::utils::logging::debug(
                "App.initiate_tool_calls",
                &format!(
                    "Executing {} already approved tools.",
                    tools_to_execute.len()
                ),
            );

            for tool_call in tools_to_execute {
                let tool_id = tool_call.id.clone();
                let tool_name = tool_call.name.clone();

                self.emit_event(AppEvent::ToolCallStarted {
                    name: tool_name,
                    id: tool_id.clone(),
                });

                let tool_executor = self.tool_executor.clone();
                let conversation = self.conversation.clone();
                let event_sender = Some(self.event_sender.clone());
                let internal_event_sender = Some(self.internal_event_sender.clone());

                // Spawn the task and store its handle
                let handle = tokio::spawn(execute_tool_and_handle_result(
                    tool_call,
                    batch_id,
                    tool_executor,
                    conversation,
                    event_sender,
                    internal_event_sender,
                ));

                // Store the handle with its batch ID
                self.active_tool_tasks.insert(tool_id, (handle, batch_id));
            }
        } else {
            crate::utils::logging::debug(
                "App.initiate_tool_calls",
                "No tools were immediately ready for execution (all need approval or none exist).",
            );
        }

        Ok(())
    }

    pub async fn handle_batch_progress(
        &mut self,
        batch_id: usize,
        tool_call_id: String,
    ) -> Result<()> {
        crate::utils::logging::debug(
            "App.handle_batch_progress",
            &format!(
                "Received progress update for batch {} from tool {}",
                batch_id, tool_call_id
            ),
        );

        // Remove the completed task from active_tool_tasks
        if self.active_tool_tasks.remove(&tool_call_id).is_some() {
            crate::utils::logging::debug(
                "App.handle_batch_progress",
                &format!(
                    "Removed completed task {} from active tracking",
                    tool_call_id
                ),
            );
        }

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
                batch_completed = true;
            }
        } else {
            crate::utils::logging::warn(
                "App.handle_batch_progress",
                &format!(
                    "Received progress for unknown or already completed batch ID: {}",
                    batch_id
                ),
            );
            return Ok(());
        }

        if should_check_completion {
            crate::utils::logging::info(
                "App.handle_batch_progress",
                &format!("Batch {} potentially complete, checking results.", batch_id),
            );
            if batch_completed {
                self.tool_batches.remove(&batch_id);
                crate::utils::logging::info(
                    "App.handle_batch_progress",
                    &format!("Removed completed batch {} from tracking.", batch_id),
                );
            }
            self.check_batch_completion(batch_id).await?;
        }
        Ok(())
    }

    async fn check_batch_completion(&mut self, batch_id: usize) -> Result<()> {
        crate::utils::logging::info(
            "app.check_batch_completion",
            &format!(
                "Processing completed batch {}. Getting results (cancellable).",
                batch_id
            ),
        );

        self.emit_event(AppEvent::ThinkingStarted);

        let tools = Some(crate::api::tools::Tool::all());
        let api_client = self.api_client.clone();
        let conversation = self.conversation.clone();
        let notifier = self.cancellation_notifier.clone();

        tokio::select! {
            biased;

             _ = notifier.notified() => {
                crate::utils::logging::info("App.check_batch_completion", "API call cancelled via notification.");
                // ThinkingCompleted should have been emitted by cancel_current_processing
                Ok(())
            }

            response_result = self.get_claude_response(conversation, api_client, tools.as_ref()) => {
                 match response_result {
                    Ok(response) => {
                        crate::utils::logging::info("App.check_batch_completion", "Received response after tool results.");

                        // Process the successful response (similar to handle_response)
                        let response_text = response.extract_text();
                        let new_tool_calls = response.extract_tool_calls();
                        let mut content_blocks: Vec<MessageContentBlock> = Vec::new();

                        if !response_text.trim().is_empty() {
                             content_blocks.push(MessageContentBlock::Text(response_text));
                        }

                        if !new_tool_calls.is_empty() {
                             let tool_call_blocks: Vec<ToolCall> = new_tool_calls.iter()
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
                           let mut conv_guard = self.conversation.lock().await;
                           conv_guard.add_message_with_blocks(Role::Assistant, content_blocks.clone());
                           let added_message_id = conv_guard.messages.last().map_or_else(|| "unknown_id".to_string(), |m| m.id.clone());
                           drop(conv_guard);
                           self.emit_event(AppEvent::MessageAdded {
                               role: Role::Assistant,
                               content_blocks,
                               id: added_message_id,
                           });
                        }

                        if !new_tool_calls.is_empty() {
                            self.initiate_tool_calls(new_tool_calls).await?;
                        } else {
                            self.emit_event(AppEvent::ThinkingCompleted);
                        }
                        Ok(())
                    }
                    Err(e) => {
                        crate::utils::logging::error("App.check_batch_completion", &format!("API call failed after tools: {}", e));
                        self.emit_event(AppEvent::ThinkingCompleted);
                        self.emit_event(AppEvent::Error { message: format!("Error processing tool results: {}", e) });
                        Err(e)
                    }
                 }
             }
        }
    }

    pub async fn handle_tool_command_response(
        &mut self,
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
                if always_approve {
                    crate::utils::logging::debug(
                        "App.handle_tool_command_response",
                        &format!("Adding tool '{}' to always-approved list.", tool_name),
                    );
                    self.approved_tools.insert(tool_name.clone());
                }

                self.emit_event(AppEvent::ToolCallStarted {
                    name: tool_name.clone(),
                    id: tool_call_id.clone(),
                });

                let tool_executor = self.tool_executor.clone();
                let conversation = self.conversation.clone();
                let event_sender = Some(self.event_sender.clone());
                let internal_event_sender = Some(self.internal_event_sender.clone());

                // Spawn the task and store its handle
                let handle = tokio::spawn(execute_tool_and_handle_result(
                    tool_call,
                    batch_id,
                    tool_executor,
                    conversation,
                    event_sender,
                    internal_event_sender,
                ));

                // Store the handle with its batch ID
                self.active_tool_tasks
                    .insert(tool_call_id.clone(), (handle, batch_id));
            } else {
                crate::utils::logging::info(
                    "App.handle_tool_command_response",
                    &format!("Tool call '{}' was denied by the user.", tool_name),
                );
                let result_content = format!("Tool '{}' denied by user.", tool_name);

                self.conversation
                    .lock()
                    .await
                    .add_tool_result(tool_call_id.clone(), result_content.clone());

                self.emit_event(AppEvent::ToolCallFailed {
                    name: tool_name,
                    error: "Denied by user".to_string(),
                    id: tool_call_id.clone(),
                });

                if let Err(e) = self
                    .internal_event_sender
                    .try_send(AppEvent::ToolBatchProgress {
                        batch_id,
                        tool_call_id: tool_call_id.clone(),
                    })
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

    pub async fn dispatch_agent(&self, prompt: &str) -> Result<String> {
        let agent =
            crate::tools::dispatch_agent::DispatchAgent::with_api_key(self.config.api_key.clone());
        agent.execute(prompt).await
    }

    pub async fn handle_command(&mut self, command: &str) -> Result<String> {
        let parts: Vec<&str> = command.trim_start_matches('/').splitn(2, ' ').collect();
        let command_name = parts[0];
        let args = parts.get(1).unwrap_or(&"").trim();

        match command_name {
            "clear" => {
                self.conversation.lock().await.clear();
                self.memory.clear()?;
                Ok("Conversation and memory cleared.".to_string())
            }
            "load" => {
                self.memory.load()?;
                Ok("Memory loaded from file.".to_string())
            }
            "save" => {
                self.memory.save()?;
                Ok("Memory saved to file.".to_string())
            }
            "compact" => {
                self.compact_conversation().await?;
                Ok("Conversation compacted.".to_string())
            }
            "dispatch" => {
                if args.is_empty() {
                    return Ok("Usage: /dispatch <prompt for agent>".to_string());
                }
                let response = self.dispatch_agent(args).await?;
                self.add_message(Message::new_text(
                    Role::Assistant,
                    format!("Dispatch Agent Result:\\n{}", response),
                ))
                .await;
                Ok(format!("Dispatch agent executed. Response added."))
            }
            _ => Ok(format!("Unknown command: {}", command_name)),
        }
    }

    pub async fn compact_conversation(&mut self) -> Result<()> {
        crate::utils::logging::info("App.compact_conversation", "Compacting conversation...");

        let current_summary = {
            let conv = self.conversation.lock().await;
            format!(
                "Conversation up to now:
{:?}",
                conv.messages
            )
        };
        self.add_to_memory("conversation_history", &current_summary)?;

        crate::utils::logging::info("App.compact_conversation", "Conversation compacted.");
        Ok(())
    }

    pub fn add_to_memory(&mut self, section: &str, content: &str) -> Result<()> {
        self.memory.add_section(section, content)?;
        Ok(())
    }

    pub fn get_from_memory(&self, section: &str) -> Option<String> {
        self.memory.get_section(section)
    }

    pub fn has_memory_file(&self) -> bool {
        self.memory.exists()
    }

    pub fn memory_content(&self) -> String {
        self.memory.content().to_string()
    }

    pub fn cancel_current_processing(&mut self) {
        let mut tools_cancelled = false;

        // Cancel active tool tasks
        if !self.active_tool_tasks.is_empty() {
            crate::utils::logging::info(
                "App.cancel_current_processing",
                &format!(
                    "Cancelling {} active tool tasks.",
                    self.active_tool_tasks.len()
                ),
            );
            tools_cancelled = true;

            let affected_batch_ids: HashSet<usize> = self
                .active_tool_tasks
                .values()
                .map(|(_, batch_id)| *batch_id)
                .collect();

            for (tool_id, (handle, _)) in self.active_tool_tasks.drain() {
                handle.abort();
                crate::utils::logging::debug(
                    "App.cancel_current_processing",
                    &format!("Aborted tool task with ID: {}", tool_id),
                );
            }

            for batch_id in affected_batch_ids {
                self.tool_batches.remove(&batch_id);
                crate::utils::logging::info(
                    "App.cancel_current_processing",
                    &format!("Removed batch {} due to cancellation", batch_id),
                );
            }

            if !self.pending_tool_calls.is_empty() {
                let count = self.pending_tool_calls.len();
                self.pending_tool_calls.clear();
                crate::utils::logging::info(
                    "App.cancel_current_processing",
                    &format!("Cleared {} pending tool calls due to cancellation", count),
                );
            }
        }

        // Notify any waiting API call future
        crate::utils::logging::debug(
            "App.cancel_current_processing",
            "Sending cancellation notification.",
        );
        self.cancellation_notifier.notify_waiters();

        // Emit ThinkingCompleted if tools were cancelled OR if no tools were running
        if tools_cancelled || self.active_tool_tasks.is_empty() {
            crate::utils::logging::debug(
                "App.cancel_current_processing",
                "Emitting ThinkingCompleted due to cancellation.",
            );
            self.emit_event(AppEvent::ThinkingCompleted);
        } else {
            // If only an API call was potentially running, notified() branch in select! handles outcome
            crate::utils::logging::debug(
                "App.cancel_current_processing",
                "No active tool tasks cancelled, notification sent for potential API call.",
            );
        }
    }

    // Simplified get_claude_response (doesn't need to be public)
    async fn get_claude_response(
        &self,
        conversation: Arc<Mutex<Conversation>>,
        api_client: crate::api::Client,
        tools: Option<&Vec<crate::api::Tool>>,
    ) -> Result<crate::api::CompletionResponse> {
        let conversation_guard = conversation.lock().await;
        let (api_messages, system_prompt_content) =
            crate::api::messages::convert_conversation(&conversation_guard);
        drop(conversation_guard);
        api_client
            .complete(api_messages, system_prompt_content, tools.cloned())
            .await
    }
}
