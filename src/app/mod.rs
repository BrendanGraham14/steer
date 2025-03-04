use anyhow::Result;
use std::sync::Arc;

mod conversation;
mod environment;
mod tool_executor;

pub use conversation::{Conversation, Message, Role};
pub use environment::EnvironmentInfo;
pub use tool_executor::ToolExecutor;

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
        })
    }
    
    /// Run the application
    pub async fn run(&mut self) -> Result<()> {
        // Initialize TUI
        let mut tui = crate::tui::Tui::new()?;
        
        // Run the main event loop
        tui.run(self).await?;
        
        Ok(())
    }
    
    /// Add a user message to the conversation
    pub fn add_user_message(&mut self, content: String) {
        self.conversation.add_message(Role::User, content);
    }
    
    /// Add an assistant message to the conversation
    pub fn add_assistant_message(&mut self, content: String) {
        self.conversation.add_message(Role::Assistant, content);
    }
    
    /// Get the current conversation
    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }
    
    /// Get the environment information
    pub fn environment_info(&self) -> &EnvironmentInfo {
        &self.env_info
    }
    
    /// Send a message to Claude and get a response
    pub async fn send_message_to_claude(&mut self, message: String) -> Result<String> {
        // Add message to conversation
        self.add_user_message(message);
        
        // Get tools
        let tools = Some(crate::api::tools::Tool::all());
        
        // Get response
        let response = self.get_claude_response(Some(&tools.as_ref().unwrap())).await?;
        
        // Check for tool calls
        if response.has_tool_calls() {
            // Extract tool calls
            let tool_calls = response.extract_tool_calls();
            
            // Execute all tool calls
            for tool_call in &tool_calls {
                // Execute the tool
                match self.execute_tool(tool_call).await {
                    Ok(result) => {
                        // Add tool result to the conversation
                        self.conversation.add_message(Role::Tool, 
                            format!("Tool result from {}: {}", tool_call.name, result));
                    },
                    Err(e) => {
                        // Log the error
                        self.conversation.add_message(Role::Tool, 
                            format!("Error executing tool {}: {}", tool_call.name, e));
                    }
                }
            }
            
            // Continue the conversation with the tool results
            let final_response = self.get_claude_response(Some(&tools.as_ref().unwrap())).await?;
            
            // Add the final response to the conversation
            let text = final_response.extract_text();
            self.add_assistant_message(text.clone());
            
            Ok(text)
        } else {
            // Add the response to the conversation
            let text = response.extract_text();
            self.add_assistant_message(text.clone());
            
            Ok(text)
        }
    }
    
    /// Execute a tool call
    pub async fn execute_tool(&self, tool_call: &crate::api::ToolCall) -> Result<String> {
        self.tool_executor.execute_tool(tool_call).await
    }
    
    /// Execute multiple tool calls in parallel
    pub async fn execute_tools(&self, tool_calls: Vec<crate::api::ToolCall>) -> std::collections::HashMap<String, Result<String>> {
        self.tool_executor.execute_tools(tool_calls).await
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