use schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::tools::ToolError;
use coder_macros::tool;

#[derive(Deserialize, Debug, JsonSchema)]
struct FetchParams {
    /// The URL to fetch content from
    url: String,
}

tool! {
    FetchTool {
        params: FetchParams,
        description: "Fetch the contents of a URL",
        name: "fetch"
    }

    async fn run(
        _tool: &FetchTool,
        params: FetchParams,
        token: Option<CancellationToken>,
    ) -> Result<String, ToolError> {
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
                        anyhow::anyhow!("HTTP error: {} when fetching URL: {}", status, url)
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
                    Ok(content) => Ok(content),
                    Err(e) => Err(ToolError::execution(
                        "Fetch",
                        anyhow::anyhow!("Failed to read response body from {}: {}", url, e)
                    )),
                }
            }
            Err(e) => Err(ToolError::execution(
                "Fetch",
                anyhow::anyhow!("Request to URL {} failed: {}", params.url, e)
            )),
        }
    }
}