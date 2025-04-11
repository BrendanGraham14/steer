use anyhow::Result;
use futures_util::future::BoxFuture;
use tokio::sync::mpsc::{self, Receiver, Sender};

pub mod conversation;
mod environment;
mod memory;
mod tool_executor;

pub use conversation::{Conversation, Message, MessageContent, Role, ToolCall};
pub use environment::EnvironmentInfo;
pub use memory::MemoryManager;
pub use tool_executor::ToolExecutor;

/// Events emitted by the App to update the UI
#[derive(Debug, Clone)]
pub enum AppEvent {
    MessageAdded {
        role: Role,
        content: String,
        id: String,
    },
    MessageUpdated {
        id: String,
        content: String,
    },
    ToolCallStarted {
        name: String,
        id: Option<String>,
    },
    ToolCallCompleted {
        name: String,
        result: String,
        id: Option<String>,
    },
    ToolCallFailed {
        name: String,
        error: String,
        id: Option<String>,
    },
    ThinkingStarted,
    ThinkingCompleted,
    CommandResponse {
        content: String,
        id: Option<String>,
    },
    ToggleMessageTruncation {
        id: String,
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

/// Main application state
pub struct App {
    pub config: AppConfig,
    pub conversation: Conversation,
    pub env_info: EnvironmentInfo,
    pub tool_executor: ToolExecutor,
    pub api_client: crate::api::Client,
    pub memory: MemoryManager,
    pub command_filter: Option<crate::tools::command_filter::CommandFilter>,
    event_sender: Option<Sender<AppEvent>>,
}

impl App {
    /// Create a new application instance
    pub fn new(config: AppConfig) -> Result<Self> {
        let env_info = EnvironmentInfo::collect()?;
        let conversation = Conversation::new();
        let tool_executor = ToolExecutor::new();
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
            event_sender: None,
        })
    }

    /// Set up the event channel for UI updates
    pub fn setup_event_channel(&mut self) -> Receiver<AppEvent> {
        let (tx, rx) = mpsc::channel(100);
        self.event_sender = Some(tx);
        rx
    }

    /// Emit an event to update the UI
    fn emit_event(&self, event: AppEvent) {
        if let Some(sender) = &self.event_sender {
            // Since this is a fire-and-forget scenario, we can log send errors
            match sender.try_send(event.clone()) {
                Ok(_) => {
                    // Log successful event sending for debugging
                    if let AppEvent::MessageAdded { role, content, id } = &event {
                        crate::utils::logging::debug(
                            "app.emit_event",
                            &format!(
                                "Sent MessageAdded event, role: {:?}, id: {}, content length: {}",
                                role,
                                id,
                                content.len()
                            ),
                        );
                    } else {
                        crate::utils::logging::debug(
                            "app.emit_event",
                            &format!("Sent event: {:?}", event),
                        );
                    }
                }
                Err(e) => {
                    crate::utils::logging::error(
                        "app.emit_event",
                        &format!("Failed to send event: {:?}", e),
                    );
                }
            }
        } else {
            crate::utils::logging::error("app.emit_event", "No event sender available");
        }
    }

    /// Run the application
    pub async fn run(&mut self) -> Result<()> {
        // Initialize TUI
        let mut tui = crate::tui::Tui::new()?;

        // Set up event channel
        let event_receiver = self.setup_event_channel();

        // Run the main event loop
        tui.run(self, event_receiver).await?;

        Ok(())
    }

    pub fn add_user_message(&mut self, content: String) {
        let message = Message::new(Role::User, content);
        self.add_message(message);
    }

    pub fn add_assistant_message(&mut self, content: String) {
        let message = Message::new(Role::Assistant, content);
        self.add_message(message);
    }

    pub fn add_tool_message(&mut self, content: String) {
        let message = Message::new(Role::Tool, content);
        self.add_message(message);
    }

    pub fn add_system_message(&mut self, content: String) {
        let message = Message::new(Role::System, content);
        self.add_message(message);
    }

    pub fn add_message(&mut self, message: Message) {
        self.conversation.messages.push(message.clone());
        // Only emit MessageAdded for non-tool messages
        // Tool messages will be handled by ToolCallStarted/Completed/Failed events
        if message.role != Role::Tool {
            self.emit_event(AppEvent::MessageAdded {
                role: message.role,
                content: message.content_string(),
                id: message.id,
            });
        }
    }

    /// Get the current conversation
    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    /// Get the environment information
    pub fn environment_info(&self) -> &EnvironmentInfo {
        &self.env_info
    }

    /// Process a user message and handle the entire flow
    pub async fn process_user_message(&mut self, message: String) -> Result<()> {
        // Add user message to conversation
        self.add_user_message(message.clone());

        // Special command handling
        if message.starts_with("/") {
            let response = self.handle_command(&message).await?;
            self.emit_event(AppEvent::CommandResponse {
                content: response.clone(),
                id: None,
            });
            return Ok(());
        }

        // Signal that we're thinking
        self.emit_event(AppEvent::ThinkingStarted);

        // Get the response from Claude (non-streaming for simplicity)
        let result = self.handle_response().await;

        // Signal that thinking is complete
        self.emit_event(AppEvent::ThinkingCompleted);

        // Handle any errors
        if let Err(e) = result {
            self.emit_event(AppEvent::Error {
                message: e.to_string(),
            });
            return Err(e);
        }

        Ok(())
    }

    /// Handle non-streaming response from Claude
    fn handle_response<'a>(&'a mut self) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Get tools
            let tools = Some(crate::api::tools::Tool::all());

            crate::utils::logging::debug(
                "app.handle_response",
                "Getting complete response from Claude (non-streaming version)",
            );

            // Get a complete response without streaming
            let complete_response = self
                .get_claude_response(Some(&tools.as_ref().unwrap()))
                .await?;

            // Get the text content from the response
            let response_text = complete_response.extract_text();

            // Log the complete response
            crate::utils::logging::debug(
                "app.handle_response",
                &format!(
                    "Received complete response with {} characters",
                    response_text.len()
                ),
            );

            // Add the assistant message with the complete response text *only* if there is text
            if !response_text.trim().is_empty() {
                crate::utils::logging::debug(
                    "app.handle_response",
                    "Adding assistant message with response text",
                );
                self.add_assistant_message(response_text);
            } else if complete_response.has_tool_calls() {
                // If there's no text but there are tool calls, we still need an assistant message
                // to attach the tool calls to in the conversation history.
                // The conversion logic will handle turning this into the correct API format.
                crate::utils::logging::debug(
                    "app.handle_response",
                    "Adding assistant message for tool calls (no text)",
                );
                // Add message with ToolCalls content
                let tool_calls_for_conv: Vec<ToolCall> = complete_response
                    .extract_tool_calls()
                    .into_iter()
                    .map(|api_tc| ToolCall {
                        id: api_tc.id.unwrap_or_default(), // Ensure ID exists
                        name: api_tc.name,
                        parameters: api_tc.parameters,
                    })
                    .collect();

                if !tool_calls_for_conv.is_empty() {
                    self.conversation.add_message_with_content(
                        Role::Assistant,
                        MessageContent::ToolCalls(tool_calls_for_conv),
                    );
                } else {
                    crate::utils::logging::warn(
                        "app.handle_response",
                        "Response indicated tool calls, but none were extracted.",
                    );
                }
            }

            // For debugging, dump all message IDs in the conversation
            let all_messages: Vec<(usize, &str, &Role)> = self
                .conversation
                .messages
                .iter()
                .enumerate()
                .map(|(idx, m)| (idx, m.id.as_str(), &m.role))
                .collect();
            crate::utils::logging::debug(
                "app.handle_response",
                &format!("All messages after adding response: {:?}", all_messages),
            );

            // Check for and process any tool calls
            if complete_response.has_tool_calls() {
                crate::utils::logging::debug(
                    "app.handle_response",
                    "Found tool calls in the response, processing them",
                );
                // Process tool calls - this will add the ToolResult messages
                self.process_tool_calls(&complete_response).await?;
            } else {
                crate::utils::logging::debug(
                    "app.handle_response",
                    "No tool calls found in the response",
                );
            }

            Ok(())
        })
    }

    /// Process tool calls from Claude's response
    fn process_tool_calls<'a>(
        &'a mut self,
        response: &'a crate::api::CompletionResponse,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Extract tool calls
            let tool_calls = response.extract_tool_calls();

            // Exit early if no tool calls
            if tool_calls.is_empty() {
                return Ok(());
            }

            // Execute all tool calls
            for tool_call in &tool_calls {
                // Signal that we're starting a tool call
                self.emit_event(AppEvent::ToolCallStarted {
                    name: tool_call.name.clone(),
                    id: tool_call.id.clone(),
                });

                // Execute the tool
                match self.execute_tool(tool_call).await {
                    Ok(result) => {
                        // Signal that the tool call completed
                        self.emit_event(AppEvent::ToolCallCompleted {
                            name: tool_call.name.clone(),
                            result: result.clone(),
                            id: tool_call.id.clone(),
                        });
                        // Collect tool result if we have an ID
                        if let Some(tool_id) = &tool_call.id {
                            // Add tool result directly to the conversation
                            self.conversation.add_tool_result(tool_id.clone(), result);
                        }
                    }
                    Err(e) => {
                        // Log the error
                        let error_message =
                            format!("Error executing tool {}: {}", tool_call.name, e);
                        crate::utils::logging::error("app.process_tool_calls", &error_message);

                        // Add the error as a ToolResult to the conversation
                        if let Some(tool_id) = &tool_call.id {
                            // Use a standard prefix for error results
                            let error_result_content = format!("Error: {}", e.to_string());
                            self.conversation
                                .add_tool_result(tool_id.clone(), error_result_content);
                        } else {
                            crate::utils::logging::error(
                                "app.process_tool_calls",
                                &format!(
                                    "Tool call {} failed but had no ID, cannot add ToolResult",
                                    tool_call.name
                                ),
                            );
                        }

                        // Signal that the tool call failed
                        self.emit_event(AppEvent::ToolCallFailed {
                            name: tool_call.name.clone(),
                            error: e.to_string(),
                            id: tool_call.id.clone(),
                        });
                    }
                }
            }

            // Process tool results - this now means sending the conversation *with* results back to Claude
            // Check if any tool results were actually added (ignore errors for now)
            let has_tool_results = self
                .conversation
                .messages
                .iter()
                .any(|m| m.role == Role::Tool);

            if has_tool_results {
                // We have tool results in the conversation, so send the updated conversation back to Claude
                crate::utils::logging::debug(
                    "app.process_tool_calls",
                    "Conversation updated with tool results, continuing interaction.",
                );
                // Continue the conversation with the tool results included
                // The handle_response function will be called again, triggering get_claude_response
                // which reads the current state of self.conversation
                // Note: Ensure no infinite loops if Claude keeps calling tools without progress.
                // Consider adding a turn limit or other guards.
                self.handle_response().await?;
            } else {
                crate::utils::logging::debug(
                    "app.process_tool_calls",
                    "No successful tool results were added, ending turn.",
                );
                // If no successful results (only errors without IDs perhaps), we might stop here.
            }

            Ok(())
        })
    }

    /// Execute a tool call
    pub async fn execute_tool(&self, tool_call: &crate::api::ToolCall) -> Result<String> {
        self.tool_executor.execute_tool(tool_call).await
    }

    /// Execute multiple tool calls in parallel
    pub async fn execute_tools(
        &self,
        tool_calls: Vec<crate::api::ToolCall>,
    ) -> std::collections::HashMap<String, Result<String>> {
        self.tool_executor.execute_tools(tool_calls).await
    }

    /// Use the dispatch agent to search or gather information
    pub async fn dispatch_agent(&self, prompt: &str) -> Result<String> {
        // Generate a unique ID for this dispatch agent call
        let agent_id = format!(
            "agent_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("Time went backwards")
                .as_secs()
        );

        // Signal that we're executing a dispatch agent
        self.emit_event(AppEvent::ToolCallStarted {
            name: "dispatch_agent".to_string(),
            id: Some(agent_id.clone()),
        });

        // Create a DispatchAgent instance
        // Choose between methods based on whether API key is available
        let agent = if self.config.api_key.is_empty() {
            crate::tools::dispatch_agent::DispatchAgent::new()
        } else {
            crate::tools::dispatch_agent::DispatchAgent::with_api_key(self.config.api_key.clone())
        };

        // Execute the agent with the prompt
        match agent.execute(prompt).await {
            Ok(result) => {
                // Signal that the dispatch agent completed successfully
                self.emit_event(AppEvent::ToolCallCompleted {
                    name: "dispatch_agent".to_string(),
                    result: result.clone(),
                    id: Some(agent_id),
                });

                // Return the result
                Ok(result)
            }
            Err(e) => {
                // Signal that the dispatch agent failed
                self.emit_event(AppEvent::ToolCallFailed {
                    name: "dispatch_agent".to_string(),
                    error: e.to_string(),
                    id: Some(agent_id),
                });

                // Propagate the error
                Err(e)
            }
        }
    }

    pub async fn get_claude_response(
        &self,
        tools: Option<&Vec<crate::api::Tool>>,
    ) -> Result<crate::api::CompletionResponse> {
        // Convert conversation to API messages using the helper function
        let (messages, system_content) =
            crate::api::messages::convert_conversation(&self.conversation);

        self.api_client
            .complete(messages, system_content, tools.cloned())
            .await
    }

    /// Handle a command
    pub async fn handle_command(&mut self, command: &str) -> Result<String> {
        // Split the command into parts
        let parts: Vec<&str> = command.split_whitespace().collect();
        let cmd = parts[0];

        match cmd {
            "/help" => {
                Ok("Available commands:\n/help - Show this help message\n/compact - Compact the conversation history\n/clear - Clear the conversation history\n/memory - Manage the memory file\n/exit - Exit the application".to_string())
            }
            "/compact" => {
                // Compact the conversation
                self.compact_conversation().await?;
                Ok("Conversation compacted".to_string())
            }
            "/clear" => {
                self.conversation.clear();
                Ok("Conversation cleared".to_string())
            }
            "/memory" => {
                // Handle memory commands
                if parts.len() < 2 {
                    // Just show memory content
                    if self.has_memory_file() {
                        Ok(format!("Memory file content:\n{}", self.memory_content()))
                    } else {
                        Ok("No memory file exists yet. Use /memory add <section> <content> to create one.".to_string())
                    }
                } else {
                    // Sub-command for memory
                    match parts[1] {
                        "add" => {
                            if parts.len() < 4 {
                                Ok("Usage: /memory add <section> <content>".to_string())
                            } else {
                                let section = parts[2];
                                let content = parts[3..].join(" ");
                                self.add_to_memory(section, &content)?;
                                Ok(format!("Added section '{}' to memory file", section))
                            }
                        }
                        "get" => {
                            if parts.len() < 3 {
                                Ok("Usage: /memory get <section>".to_string())
                            } else {
                                let section = parts[2];
                                if let Some(content) = self.get_from_memory(section) {
                                    Ok(format!("Section '{}':\n{}", section, content))
                                } else {
                                    Ok(format!("Section '{}' not found", section))
                                }
                            }
                        }
                        _ => {
                            Ok(format!("Unknown memory command: {}. Available commands: add, get", parts[1]))
                        }
                    }
                }
            }
            _ => {
                Ok(format!("Unknown command: {}", command))
            }
        }
    }

    pub async fn compact_conversation(&mut self) -> Result<()> {
        // TODO: Update prompt

        let compaction_messages = vec![
            crate::api::Message::new_user(format!(
                "Summarize the following conversation to preserve important context while reducing token usage.
                Include key information, decisions, and context needed for future interactions.

                {}",
                self.conversation.to_string()
            ))
        ];

        let summary = self
            .api_client
            .complete(compaction_messages, None, None)
            .await?;

        // Replace the current conversation with a compacted version
        let system_prompt = self.conversation.system_prompt();
        self.conversation.clear();

        // Re-add the system prompt if available
        if let Some(system_message) = system_prompt {
            self.conversation.add_system_message(system_message);
        }

        // Add the summary as a system message
        self.conversation.add_system_message(format!(
            "CONVERSATION HISTORY SUMMARY:\n{}",
            summary.extract_text()
        ));

        Ok(())
    }

    /// Add information to the memory file
    pub fn add_to_memory(&mut self, section: &str, content: &str) -> Result<()> {
        // Add the section to the memory file
        self.memory.add_section(section, content)?;

        // Emit an event to notify the user
        self.emit_event(AppEvent::CommandResponse {
            content: format!("Added to CLAUDE.md - Section: {}", section),
            id: None,
        });

        Ok(())
    }

    /// Get information from the memory file
    pub fn get_from_memory(&self, section: &str) -> Option<String> {
        self.memory.get_section(section)
    }

    /// Check if the memory file exists
    pub fn has_memory_file(&self) -> bool {
        self.memory.exists()
    }

    /// Get the entire content of the memory file
    pub fn memory_content(&self) -> &str {
        self.memory.content()
    }

    // New method to trigger the toggle event
    pub fn toggle_message_truncation(&self, id: String) {
        crate::utils::logging::debug(
            "app.toggle_message_truncation",
            &format!(
                "Attempting to emit ToggleMessageTruncation event for ID: {}",
                id
            ),
        );
        self.emit_event(AppEvent::ToggleMessageTruncation { id });
    }
}
