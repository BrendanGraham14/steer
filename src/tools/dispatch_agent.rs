use anyhow::{Context, Result};
use std::env;

/// Dispatch Agent implementation
pub struct DispatchAgent {
    api_key: String,
}

impl DispatchAgent {
    pub fn new() -> Self {
        // Default implementation gets the API key from environment
        let api_key = env::var("CLAUDE_API_KEY")
            .unwrap_or_else(|_| String::from(""));
        
        Self {
            api_key
        }
    }
    
    pub fn with_api_key(api_key: String) -> Self {
        Self {
            api_key
        }
    }

    /// Execute the dispatch agent with a prompt
    pub async fn execute(&self, prompt: &str) -> Result<String> {
        // Make sure we have an API key
        if self.api_key.is_empty() {
            return Err(anyhow::anyhow!("No API key provided for dispatch agent"));
        }

        // Create a client for the dispatch agent
        let dispatch_client = crate::api::Client::new(&self.api_key);

        // Create a minimal set of tools available to the dispatch agent
        // Only read-only tools are available to prevent modifications
        let tools = vec![
            crate::api::Tool::glob_tool(),
            crate::api::Tool::grep_tool(),
            crate::api::Tool::ls(),
            crate::api::Tool::view(),
        ];

        // Create a system prompt for the dispatch agent
        let system_prompt = self.create_system_prompt()?;

        // Create the messages for the API call
        let messages = vec![
            crate::api::Message {
                role: "system".to_string(),
                content: system_prompt,
            },
            crate::api::Message {
                role: "user".to_string(),
                content: prompt.to_string(),
            },
        ];

        // Call the API
        let response = dispatch_client.complete(messages, Some(tools)).await?;

        // Extract the response text
        let response_text = response.extract_text();

        Ok(response_text)
    }

    /// Create the system prompt for the dispatch agent
    fn create_system_prompt(&self) -> Result<String> {
        // Get the environment information
        let env_info = crate::app::EnvironmentInfo::collect()?;
        
        // Read the dispatch agent prompt template
        let dispatch_prompt = include_str!("../../prompts/dispatch_agent.md");
        
        // Create a formatted environment info section
        let env_info_str = format!(
            "Here is useful information about the environment you are running in:\n\
            <env>\n\
            Working directory: {}\n\
            Is directory a git repo: {}\n\
            Platform: {}\n\
            Today's date: {}\n\
            </env>",
            env_info.working_directory.display(),
            if env_info.is_git_repo { "Yes" } else { "No" },
            env_info.platform,
            env_info.date
        );
        
        // Combine the prompt template with the environment info
        let full_prompt = format!("{}\n\n{}", dispatch_prompt, env_info_str);
        
        Ok(full_prompt)
    }
}