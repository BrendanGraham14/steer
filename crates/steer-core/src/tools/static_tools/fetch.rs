use async_trait::async_trait;

use crate::app::conversation::{Message, MessageData, UserContent};
use crate::config::model::builtin::claude_haiku_4_5 as summarization_model;
use crate::tools::capability::Capabilities;
use crate::tools::services::ModelCallError;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::error::ToolExecutionError;
use steer_tools::result::FetchResult;
use steer_tools::tools::fetch::{FetchError, FetchParams, FetchToolSpec};

const DESCRIPTION: &str = r#"- Fetches content from a specified URL and processes it using an AI model
- Takes a URL and a prompt as input
- Fetches the URL content and passes it to a small, fast model for analysis
- Returns the model's response about the content
- Use this tool when you need to retrieve and analyze web content

Usage notes:
  - IMPORTANT: If an MCP-provided web fetch tool is available, prefer using that tool instead of this one, as it may have fewer restrictions. All MCP-provided tools start with "mcp__".
  - The URL must be a fully-formed valid URL
  - HTTP URLs will be automatically upgraded to HTTPS
  - For security reasons, the URL's domain must have been provided directly by the user, unless it's on a small pre-approved set of the top few dozen hosts for popular coding resources, like react.dev.
  - The prompt should describe what information you want to extract from the page
  - This tool is read-only and does not modify any files
  - Results may be summarized if the content is very large"#;

pub struct FetchTool;

#[async_trait]
impl StaticTool for FetchTool {
    type Params = FetchParams;
    type Output = FetchResult;
    type Spec = FetchToolSpec;

    const DESCRIPTION: &'static str = DESCRIPTION;
    const REQUIRES_APPROVAL: bool = true;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::from_bits_truncate(
        Capabilities::NETWORK.bits() | Capabilities::MODEL_CALLER.bits(),
    );

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let model_caller = ctx
            .services
            .model_caller()
            .ok_or_else(|| StaticToolError::missing_capability("model_caller"))?;

        let content = fetch_url(&params.url, &ctx.cancellation_token).await?;

        let user_message = format!(
            r#"Web page content:
---
{content}
---

{}

Provide a concise response based only on the content above.
"#,
            params.prompt
        );

        let messages = vec![Message {
            data: MessageData::User {
                content: vec![UserContent::Text { text: user_message }],
            },
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("user", Message::current_timestamp()),
            parent_message_id: None,
        }];

        let response = model_caller
            .call(
                &summarization_model(),
                messages,
                None,
                ctx.cancellation_token.clone(),
            )
            .await
            .map_err(|e| match e {
                ModelCallError::Api(msg) => StaticToolError::execution(ToolExecutionError::Fetch(
                    FetchError::ModelCallFailed { message: msg },
                )),
                ModelCallError::Cancelled => StaticToolError::Cancelled,
            })?;

        let result_content = response.extract_text().trim().to_string();

        Ok(FetchResult {
            url: params.url,
            content: result_content,
        })
    }
}

async fn fetch_url(
    url: &str,
    token: &tokio_util::sync::CancellationToken,
) -> Result<String, StaticToolError> {
    let client = reqwest::Client::new();
    let request = client.get(url);

    let response = tokio::select! {
        result = request.send() => result,
        _ = token.cancelled() => return Err(StaticToolError::Cancelled),
    };

    match response {
        Ok(response) => {
            let status = response.status();
            let url = response.url().to_string();

            if !status.is_success() {
                return Err(StaticToolError::execution(ToolExecutionError::Fetch(
                    FetchError::Http {
                        status: status.as_u16(),
                        url,
                    },
                )));
            }

            let text = tokio::select! {
                result = response.text() => result,
                _ = token.cancelled() => return Err(StaticToolError::Cancelled),
            };

            match text {
                Ok(content) => Ok(content),
                Err(e) => Err(StaticToolError::execution(ToolExecutionError::Fetch(
                    FetchError::ReadFailed {
                        url,
                        message: e.to_string(),
                    },
                ))),
            }
        }
        Err(e) => Err(StaticToolError::execution(ToolExecutionError::Fetch(
            FetchError::RequestFailed {
                message: format!("Request to URL {url} failed: {e}"),
            },
        ))),
    }
}
