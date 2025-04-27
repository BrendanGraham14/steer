use anyhow::{Context, Result};
use serde_json::Value;

// Export the modules for testing and direct use
pub mod bash;
pub mod command_filter;
pub mod dispatch_agent;
pub mod edit;
pub mod glob_tool;
pub mod grep_tool;
pub mod ls;
pub mod replace;
pub mod view;

/// Execute a tool based on the name and parameters
pub async fn execute_tool(name: &str, parameters: &Value) -> Result<String> {
    match name {
        "Bash" => {
            let command = parameters["command"]
                .as_str()
                .context("Missing command parameter")?;

            let timeout = parameters
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(3_600_000); // Default to 1 hour

            bash::execute_bash(command, timeout).await
        }
        "GlobTool" => {
            let pattern = parameters["pattern"]
                .as_str()
                .context("Missing pattern parameter")?;

            let path = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            glob_tool::glob_search(pattern, path)
        }
        "GrepTool" => {
            let pattern = parameters["pattern"]
                .as_str()
                .context("Missing pattern parameter")?;

            let include = parameters.get("include").and_then(|v| v.as_str());

            let path = parameters
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            grep_tool::grep_search(pattern, include, path)
        }
        "LS" => {
            let path = parameters["path"]
                .as_str()
                .context("Missing path parameter")?;

            let ignore: Vec<String> = parameters
                .get("ignore")
                .and_then(|v| {
                    if v.is_array() {
                        Some(
                            v.as_array()
                                .unwrap()
                                .iter()
                                .filter_map(|p| p.as_str().map(String::from))
                                .collect(),
                        )
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            ls::list_directory(path, &ignore)
        }
        "View" => {
            let file_path = parameters["file_path"]
                .as_str()
                .context("Missing file_path parameter")?;

            let offset = parameters
                .get("offset")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);

            let limit = parameters
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);

            view::view_file(file_path, offset, limit)
        }
        "Edit" => {
            let file_path = parameters["file_path"]
                .as_str()
                .context("Missing file_path parameter")?;

            let old_string = parameters["old_string"]
                .as_str()
                .context("Missing old_string parameter")?;

            let new_string = parameters["new_string"]
                .as_str()
                .context("Missing new_string parameter")?;

            edit::edit_file(file_path, old_string, new_string)
        }
        "Replace" => {
            let file_path = parameters["file_path"]
                .as_str()
                .context("Missing file_path parameter")?;

            let content = parameters["content"]
                .as_str()
                .context("Missing content parameter")?;

            replace::replace_file(file_path, content)
        }
        "dispatch_agent" => {
            let prompt = parameters["prompt"]
                .as_str()
                .context("Missing prompt parameter")?;

            let agent = dispatch_agent::DispatchAgent::new();
            Box::pin(agent.execute(prompt)).await
        }
        _ => Err(anyhow::anyhow!("Unknown tool: {}", name)),
    }
}
