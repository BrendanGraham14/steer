use crate::config::LlmConfigProvider;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use steer_macros::tool_external as tool;
use steer_tools::ToolError;

#[derive(Deserialize, Debug, JsonSchema)]
pub struct FetchParams {
    /// The URL to fetch content from
    pub url: String,
    /// The prompt to process the content with
    pub prompt: String,
}

tool! {
    pub struct FetchTool {
        pub llm_config_provider: Arc<LlmConfigProvider>,
    } {
        params: FetchParams,
        output: steer_tools::result::FetchResult,
        variant: Fetch,
        description: r#"- Fetches content from a specified URL and processes it using an AI model
- Takes a URL and a prompt as input
- Fetches the URL content, converts HTML to markdown
- Processes the content with the prompt using a small, fast model
- Returns the model's response about the content
- Use this tool when you need to retrieve and analyze web content

Usage notes:
  - IMPORTANT: If an MCP-provided web fetch tool is available, prefer using that tool instead of this one, as it may have fewer restrictions. All MCP-provided tools start with "mcp__".
  - The URL must be a fully-formed valid URL
  - HTTP URLs will be automatically upgraded to HTTPS
  - For security reasons, the URL's domain must have been provided directly by the user, unless it's on a small pre-approved set of the top few dozen hosts for popular coding resources, like react.dev.
  - The prompt should describe what information you want to extract from the page
  - This tool is read-only and does not modify any files
  - Results may be summarized if the content is very large
  - Includes a self-cleaning 15-minute cache for faster responses when repeatedly accessing the same URL"#,
        name: "web_fetch",
        require_approval: true
    }

    async fn run(
        tool: &FetchTool,
        params: FetchParams,
        context: &steer_tools::ExecutionContext,
    ) -> Result<steer_tools::result::FetchResult, ToolError> {
        let token = Some(context.cancellation_token.clone());
        // Create a reqwest client
        let client = reqwest::Client::new();

        // Create the request
        let request = client.get(&params.url);

        // Send the request and check for cancellation
        let response = if let Some(ref token) = token {
            tokio::select! {
                result = request.send() => result,
                _ = token.cancelled() => return Err(ToolError::Cancelled("Fetch".to_string())),
            }
        } else {
            request.send().await
        };

        // Handle the response
        match response {
            Ok(response) => {
                let status = response.status();
                let url = response.url().to_string();

                if !status.is_success() {
                    return Err(ToolError::execution(
                        "Fetch",
                        format!("HTTP error: {status} when fetching URL: {url}")
                    ));
                }

                // Get the response text
                let text = if let Some(ref token) = token {
                    tokio::select! {
                        result = response.text() => result,
                        _ = token.cancelled() => return Err(ToolError::Cancelled("Fetch".to_string())),
                    }
                } else {
                    response.text().await
                };

                match text {
                    Ok(content) => {
                        process_web_page_content(tool, content, params.prompt.clone(), token).await
                            .map(|payload| steer_tools::result::FetchResult {
                                url: params.url,
                                content: payload,
                            })
                    }
                    Err(e) => Err(ToolError::execution(
                        "Fetch",
                        format!("Failed to read response body from {url}: {e}")
                    )),
                }
            }
            Err(e) => Err(ToolError::execution(
                "Fetch",
                format!("Request to URL {} failed: {}", params.url, e)
            )),
        }
    }
}

// Add is_read_only implementation outside the macro
impl FetchTool {
    pub fn is_read_only(&self) -> bool {
        true
    }
}

async fn process_web_page_content(
    tool: &FetchTool,
    content: String,
    prompt: String,
    token: Option<tokio_util::sync::CancellationToken>,
) -> Result<String, ToolError> {
    let client = crate::api::Client::new_with_provider((*tool.llm_config_provider).clone());
    let user_message = format!(
        r#"Web page content:
---
{content}
---

{prompt}

Provide a concise response based only on the content above.
"#
    );

    let messages = vec![crate::app::conversation::Message {
        data: crate::app::conversation::MessageData::User {
            content: vec![crate::app::conversation::UserContent::Text { text: user_message }],
        },
        timestamp: crate::app::conversation::Message::current_timestamp(),
        id: crate::app::conversation::Message::generate_id(
            "user",
            crate::app::conversation::Message::current_timestamp(),
        ),
        parent_message_id: None,
    }];

    let token = if let Some(ref token) = token {
        token.clone()
    } else {
        tokio_util::sync::CancellationToken::new()
    };

    match client
        .complete(
            &crate::config::model::builtin::claude_3_5_haiku_20241022(),
            messages,
            None,
            None,
            None,
            token,
        )
        .await
    {
        Ok(response) => {
            let prefix = response.extract_text();
            Ok(prefix.trim().to_string())
        }
        Err(e) => Err(ToolError::execution(
            "Fetch",
            format!("Failed to process web page content: {e}"),
        )),
    }
}
