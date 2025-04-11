use anyhow::Result;
use std::env;

/// Dispatch Agent implementation
pub struct DispatchAgent {
    api_key: String,
}

impl DispatchAgent {
    pub fn new() -> Self {
        // Default implementation gets the API key from environment
        let api_key = env::var("CLAUDE_API_KEY").unwrap_or_else(|_| String::from(""));

        Self { api_key }
    }

    pub fn with_api_key(api_key: String) -> Self {
        Self { api_key }
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
        let mut messages = vec![crate::api::Message {
            role: "user".to_string(),
            content: crate::api::messages::MessageContent::Text {
                content: prompt.to_string(),
            },
            id: None,
        }];

        // Let's start fresh with a cleaner approach
        // Call the API with the initial message
        let mut api_messages = messages.clone();
        let mut api_response = dispatch_client
            .complete(
                api_messages.clone(),
                Some(system_prompt.clone()),
                Some(tools.clone()),
            )
            .await?;

        // Process tool calls until there are no more
        while api_response.has_tool_calls() {
            // Save the assistant's full response content
            let content = api_response.extract_text();

            // Get the tool calls directly from the response
            let tool_calls = api_response.extract_tool_calls();

            // Create an array to hold all tool result content blocks
            let mut tool_results = Vec::new();

            // Process each tool call
            for tool_call in &tool_calls {
                let tool_id = match &tool_call.id {
                    Some(id) => id.clone(),
                    None => continue, // Skip tool calls without an ID
                };

                // Get the tool name and parameters
                let tool_name = &tool_call.name;
                let parameters = &tool_call.parameters;

                // Execute the tool based on its name
                let result = match tool_name.as_str() {
                    "GlobTool" => {
                        let pattern = parameters["pattern"].as_str().unwrap_or("");
                        let path = parameters
                            .get("path")
                            .and_then(|p| p.as_str())
                            .unwrap_or(".");
                        Ok(crate::tools::glob_tool::glob_search(pattern, path)?)
                    }
                    "GrepTool" => {
                        let pattern = parameters["pattern"].as_str().unwrap_or("");
                        let path = parameters
                            .get("path")
                            .and_then(|p| p.as_str())
                            .unwrap_or(".");
                        let include = parameters.get("include").and_then(|i| i.as_str());
                        Ok(crate::tools::grep_tool::grep_search(
                            pattern, include, path,
                        )?)
                    }
                    "LS" => {
                        let path = parameters["path"].as_str().unwrap_or(".");
                        let ignore = parameters
                            .get("ignore")
                            .and_then(|i| i.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_else(Vec::new);
                        Ok(crate::tools::ls::list_directory(path, &ignore)?)
                    }
                    "View" => {
                        let file_path = parameters["file_path"].as_str().unwrap_or("");
                        let offset = parameters
                            .get("offset")
                            .and_then(|o| o.as_u64())
                            .map(|o| o as usize);
                        let limit = parameters
                            .get("limit")
                            .and_then(|l| l.as_u64())
                            .map(|l| l as usize);
                        Ok(crate::tools::view::view_file(file_path, offset, limit)?)
                    }
                    _ => Err(anyhow::anyhow!("Unsupported tool: {}", tool_name)),
                };

                // Convert the result to a string
                let result_string = match result {
                    Ok(res) => res,
                    Err(e) => format!("Error executing tool {}: {}", tool_name, e),
                };

                // Add tool result to our collection
                tool_results.push(serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tool_id,
                    "content": result_string
                }));
            }

            // Create a new collection for the next API call
            // First add the original user message
            let mut new_api_messages = Vec::new();
            if !api_messages.is_empty() {
                new_api_messages.push(api_messages[0].clone());
            }

            // Add the assistant's response with the tool calls
            new_api_messages.push(crate::api::Message {
                role: "assistant".to_string(),
                content: crate::api::messages::MessageContent::Text { content: content },
                id: None,
            });

            // Add the user's tool result message
            new_api_messages.push(crate::api::Message {
                role: "user".to_string(),
                content: crate::api::messages::MessageContent::StructuredContent {
                    content: crate::api::messages::StructuredContent(vec![]),
                },
                id: None,
            });

            // Replace our messages with the new set
            api_messages = new_api_messages;

            // Call the API again with the updated message sequence
            api_response = dispatch_client
                .complete(
                    api_messages.clone(),
                    Some(system_prompt.clone()),
                    Some(tools.clone()),
                )
                .await?;
        }

        // Return the final response text
        let response_text = api_response.extract_text();

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
