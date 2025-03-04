use anyhow::Result;
use tokio::sync::mpsc::{self, Receiver, Sender};
use futures_util::future::BoxFuture;

mod conversation;
mod environment;
mod tool_executor;

pub use conversation::{Conversation, Message, Role};
pub use environment::EnvironmentInfo;
pub use tool_executor::ToolExecutor;

/// Events emitted by the App to update the UI
#[derive(Debug, Clone)]
pub enum AppEvent {
    MessageAdded {
        role: Role,
        content: String,
    },
    ToolCallStarted {
        name: String,
    },
    ToolCallCompleted {
        name: String,
        result: String,
    },
    ToolCallFailed {
        name: String,
        error: String,
    },
    ThinkingStarted,
    ThinkingCompleted,
    CommandResponse {
        content: String,
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
    event_sender: Option<Sender<AppEvent>>,
}

impl App {
    /// Create a new application instance
    pub fn new(config: AppConfig) -> Result<Self> {
        let env_info = EnvironmentInfo::collect()?;
        let conversation = Conversation::new();
        let tool_executor = ToolExecutor::new();
        let api_client = crate::api::Client::new(&config.api_key);
        
        Ok(Self {
            config,
            conversation,
            env_info,
            tool_executor,
            api_client,
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
            // Since this is a fire-and-forget scenario, we just ignore errors
            let _ = sender.try_send(event);
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
    
    /// Add a user message to the conversation
    pub fn add_user_message(&mut self, content: String) {
        self.conversation.add_message(Role::User, content.clone());
        self.emit_event(AppEvent::MessageAdded {
            role: Role::User,
            content,
        });
    }
    
    /// Add an assistant message to the conversation
    pub fn add_assistant_message(&mut self, content: String) {
        self.conversation.add_message(Role::Assistant, content.clone());
        self.emit_event(AppEvent::MessageAdded {
            role: Role::Assistant,
            content,
        });
    }
    
    /// Add a tool message to the conversation
    pub fn add_tool_message(&mut self, content: String) {
        self.conversation.add_message(Role::Tool, content.clone());
        self.emit_event(AppEvent::MessageAdded {
            role: Role::Tool,
            content,
        });
    }
    
    /// Add a system message to the conversation
    pub fn add_system_message(&mut self, content: String) {
        self.conversation.add_message(Role::System, content.clone());
        self.emit_event(AppEvent::MessageAdded {
            role: Role::System,
            content,
        });
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
                content: response.clone() 
            });
            return Ok(());
        }
        
        // Signal that we're thinking
        self.emit_event(AppEvent::ThinkingStarted);
        
        // Get the response from Claude (streaming)
        let result = self.handle_streaming_response().await;
        
        // Signal that thinking is complete
        self.emit_event(AppEvent::ThinkingCompleted);
        
        // Handle any errors
        if let Err(e) = result {
            self.emit_event(AppEvent::Error { 
                message: e.to_string() 
            });
            return Err(e);
        }
        
        Ok(())
    }
    
    /// Handle streaming response from Claude
    fn handle_streaming_response<'a>(&'a mut self) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Get tools
            let tools = Some(crate::api::tools::Tool::all());
            
            // Create a stream for the response
            let mut stream = self.get_claude_response_streaming(Some(&tools.as_ref().unwrap()));
            
            // Create a placeholder for the assistant's message
            let mut response_text = String::new();
            self.add_assistant_message(response_text.clone());
            
            // Get the index of the placeholder message
            let msg_index = self.conversation.messages.len() - 1;
            
            // Process the stream
            use futures_util::StreamExt;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(text) => {
                        // Update the message
                        response_text.push_str(&text);
                        
                        // Update the message in the conversation
                        if let Some(msg) = self.conversation.messages.get_mut(msg_index) {
                            msg.content = response_text.clone();
                        }
                        
                        // Emit an event to update the UI
                        self.emit_event(AppEvent::MessageAdded {
                            role: Role::Assistant,
                            content: response_text.clone(),
                        });
                    },
                    Err(e) => {
                        self.emit_event(AppEvent::Error { 
                            message: format!("Streaming error: {}", e) 
                        });
                        return Err(e.into());
                    }
                }
            }
            
            // Now check for tool calls in the complete response
            // We need a non-streaming response to properly parse tool calls
            let complete_response = self.get_claude_response(Some(&tools.as_ref().unwrap())).await?;
            
            if complete_response.has_tool_calls() {
                // Process tool calls
                self.process_tool_calls(&complete_response).await?;
            }
            
            Ok(())
        })
    }
    
    /// Process tool calls from Claude's response
    fn process_tool_calls<'a>(&'a mut self, response: &'a crate::api::CompletionResponse) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Extract tool calls
            let tool_calls = response.extract_tool_calls();
            
            // Execute all tool calls
            for tool_call in &tool_calls {
                // Signal that we're starting a tool call
                self.emit_event(AppEvent::ToolCallStarted { 
                    name: tool_call.name.clone() 
                });
                
                // Execute the tool
                match self.execute_tool(tool_call).await {
                    Ok(result) => {
                        // Add tool result to the conversation
                        let tool_message = format!("Tool result from {}: {}", tool_call.name, result);
                        self.add_tool_message(tool_message);
                        
                        // Signal that the tool call completed
                        self.emit_event(AppEvent::ToolCallCompleted { 
                            name: tool_call.name.clone(), 
                            result: result.clone() 
                        });
                    },
                    Err(e) => {
                        // Log the error
                        let error_message = format!("Error executing tool {}: {}", tool_call.name, e);
                        self.add_tool_message(error_message);
                        
                        // Signal that the tool call failed
                        self.emit_event(AppEvent::ToolCallFailed { 
                            name: tool_call.name.clone(), 
                            error: e.to_string() 
                        });
                    }
                }
            }
            
            // Continue the conversation with the tool results
            if !tool_calls.is_empty() {
                self.handle_streaming_response().await?;
            }
            
            Ok(())
        })
    }
    
    /// Execute a tool call
    pub async fn execute_tool(&self, tool_call: &crate::api::ToolCall) -> Result<String> {
        self.tool_executor.execute_tool(tool_call).await
    }
    
    /// Execute multiple tool calls in parallel
    pub async fn execute_tools(&self, tool_calls: Vec<crate::api::ToolCall>) -> std::collections::HashMap<String, Result<String>> {
        self.tool_executor.execute_tools(tool_calls).await
    }
    
    /// Use the dispatch agent to search or gather information
    pub async fn dispatch_agent(&self, prompt: &str) -> Result<String> {
        // Signal that we're executing a dispatch agent
        self.emit_event(AppEvent::ToolCallStarted { 
            name: "dispatch_agent".to_string() 
        });
        
        // Create a DispatchAgent instance with the app's API key
        let agent = crate::tools::dispatch_agent::DispatchAgent::with_api_key(
            self.config.api_key.clone()
        );
        
        // Execute the agent with the prompt
        match agent.execute(prompt).await {
            Ok(result) => {
                // Signal that the dispatch agent completed successfully
                self.emit_event(AppEvent::ToolCallCompleted { 
                    name: "dispatch_agent".to_string(),
                    result: result.clone() 
                });
                
                // Return the result
                Ok(result)
            },
            Err(e) => {
                // Signal that the dispatch agent failed
                self.emit_event(AppEvent::ToolCallFailed { 
                    name: "dispatch_agent".to_string(),
                    error: e.to_string() 
                });
                
                // Propagate the error
                Err(e)
            }
        }
    }
    
    /// Get a response from Claude
    pub async fn get_claude_response(&self, tools: Option<&Vec<crate::api::Tool>>) -> Result<crate::api::CompletionResponse> {
        let messages = crate::api::messages::convert_conversation(&self.conversation);
        self.api_client.complete(messages, tools.cloned()).await
    }
    
    /// Get a streaming response from Claude
    pub fn get_claude_response_streaming(&self, tools: Option<&Vec<crate::api::Tool>>) -> crate::api::CompletionStream {
        let messages = crate::api::messages::convert_conversation(&self.conversation);
        self.api_client.complete_streaming(messages, tools.cloned())
    }
    
    /// Handle a command
    pub async fn handle_command(&mut self, command: &str) -> Result<String> {
        match command {
            "/help" => {
                Ok("Available commands:\n/help - Show this help message\n/compact - Compact the conversation history\n/clear - Clear the conversation history\n/exit - Exit the application".to_string())
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
            _ => {
                Ok(format!("Unknown command: {}", command))
            }
        }
    }
    
    /// Compact the conversation
    pub async fn compact_conversation(&mut self) -> Result<()> {
        // Generate a prompt for summarizing the conversation
        let prompt = format!(
            "Summarize the following conversation to preserve important context while reducing token usage. 
            Include key information, decisions, and context needed for future interactions.
            
            {}",
            self.conversation.to_string()
        );
        
        // Get a summary
        let summary = self.api_client.generate_summary(&prompt).await?;
        
        // Replace the current conversation with a compacted version
        let system_prompt = self.conversation.system_prompt().cloned();
        self.conversation.clear();
        
        // Re-add the system prompt if available
        if let Some(system_message) = system_prompt {
            self.conversation.add_system_message(system_message);
        }
        
        // Add the summary as a system message
        self.conversation.add_system_message(format!("CONVERSATION HISTORY SUMMARY:\n{}", summary));
        
        Ok(())
    }
}