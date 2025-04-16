use anyhow::Result;
use std::env;

// Import the API client
use crate::api::Client as ApiClient;

/// Dispatch Agent implementation
pub struct DispatchAgent {
    // Store the API client instead of just the key
    api_client: ApiClient,
}

impl DispatchAgent {
    pub fn new() -> Self {
        // Default implementation gets the API key from environment
        let api_key = env::var("CLAUDE_API_KEY").unwrap_or_else(|_| String::from(""));

        // Create the API client
        let api_client = ApiClient::new(&api_key).with_model("claude-3-haiku-20240307"); // Or use a different default model if needed

        Self { api_client }
    }

    pub fn with_api_key(api_key: String) -> Self {
        // Create the API client with the provided key
        let api_client = ApiClient::new(&api_key).with_model("claude-3-haiku-20240307"); // Or use a different default model if needed
        Self { api_client }
    }

    /// Execute the dispatch agent with a prompt
    pub async fn execute(&self, prompt: &str) -> Result<String> {
        // No longer need to check api_key directly, client creation handles it (implicitly)
        // No longer need to create reqwest client here, ApiClient handles it

        let tools = vec![/* ... list of tools ... */]; // TODO: Define actual tools if needed by the dispatch agent
        let system_prompt = self.create_system_prompt()?;

        // Create the messages for the API call
        let messages = vec![crate::api::Message::new_user(prompt.to_string())];

        // Call the API using the stored api_client
        // Use 'completion' as the variable name for the response to avoid confusion
        let completion = self
            .api_client
            .complete(
                messages, // No need to clone api_messages anymore
                Some(system_prompt),
                Some(tools), // Pass None if no tools are defined/needed
            )
            .await?;

        // NOTE: The API client's `complete` method already handles non-success status codes
        // and returns a Result, so we don't need to check response.status() here.
        // The response is already parsed into `CompletionResponse` by the client.

        // Extract the response text
        let response_text = completion.extract_text();

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
            "Here is useful information about the environment you are running in:\\n\
            <env>\\n\
            Working directory: {}\\n\
            Is directory a git repo: {}\\n\
            Platform: {}\\n\
            Today's date: {}\\n\
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
