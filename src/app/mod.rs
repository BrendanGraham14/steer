use anyhow::Result;
use std::sync::Arc;

mod conversation;
mod environment;

pub use conversation::{Conversation, Message, Role};
pub use environment::EnvironmentInfo;

/// Configuration for the application
pub struct AppConfig {
    pub api_key: String,
    // Add more configuration options as needed
}

/// Main application state
pub struct App {
    config: AppConfig,
    conversation: Conversation,
    env_info: EnvironmentInfo,
    // TUI state will be added later
}

impl App {
    /// Create a new application instance
    pub fn new(config: AppConfig) -> Result<Self> {
        let env_info = EnvironmentInfo::collect()?;
        let conversation = Conversation::new();
        
        Ok(Self {
            config,
            conversation,
            env_info,
        })
    }
    
    /// Run the application
    pub async fn run(&mut self) -> Result<()> {
        // Initialize API client
        let api_client = crate::api::Client::new(&self.config.api_key);
        
        // Initialize TUI
        let mut tui = crate::tui::Tui::new()?;
        
        // Run the main event loop
        tui.run(self, api_client).await?;
        
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
}