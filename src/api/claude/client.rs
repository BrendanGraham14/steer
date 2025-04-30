use anyhow::{Context as AnyhowContext, Result};
use async_trait::async_trait;
use reqwest::{self, header};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::api::Model;
use crate::api::messages::Message;
use crate::api::provider::{CompletionResponse, ContentBlock, Provider};
use crate::api::tools::Tool;
const API_URL: &str = "https://api.anthropic.com/v1/messages";

#[derive(Clone)]
pub struct AnthropicClient {
    http_client: reqwest::Client,
}

#[derive(Debug, Serialize, Deserialize)]
struct CompletionRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ClaudeCompletionResponse {
    id: String,
    content: Vec<ClaudeContentBlock>,
    model: String,
    role: String,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    stop_sequence: Option<String>,
    #[serde(default)]
    usage: Usage,
    // Allow other fields for API flexibility
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
enum ClaudeContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    },
    // Add a catch-all variant for future API additions
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
struct Usage {
    #[serde(default)]
    input_tokens: usize,
    #[serde(default)]
    output_tokens: usize,
}

impl AnthropicClient {
    pub fn new(api_key: &str) -> Self {
        let mut headers = header::HeaderMap::new();
        headers.insert("x-api-key", header::HeaderValue::from_str(api_key).unwrap());
        headers.insert(
            "anthropic-version",
            header::HeaderValue::from_static("2023-06-01"),
        );
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("Failed to build HTTP client");

        Self {
            http_client: client,
        }
    }
}

// Convert Claude's content blocks to our provider-agnostic format
fn convert_claude_content(claude_blocks: Vec<ClaudeContentBlock>) -> Vec<ContentBlock> {
    claude_blocks
        .into_iter()
        .map(|block| match block {
            ClaudeContentBlock::Text { text, extra } => ContentBlock::Text { text, extra },
            ClaudeContentBlock::ToolUse {
                id,
                name,
                input,
                extra,
            } => ContentBlock::ToolUse {
                id,
                name,
                input,
                extra,
            },
            ClaudeContentBlock::Unknown => ContentBlock::Unknown,
        })
        .collect()
}

#[async_trait]
impl Provider for AnthropicClient {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn complete(
        &self,
        model: Model,
        messages: Vec<Message>,
        system: Option<String>,
        tools: Option<Vec<Tool>>,
        token: CancellationToken,
    ) -> Result<CompletionResponse> {
        let request = CompletionRequest {
            model: model.as_ref().to_string(),
            messages,
            max_tokens: 4000,
            system,
            tools,
            temperature: Some(0.7),
            top_p: None,
            top_k: None,
            stream: None,
        };

        crate::utils::logging::debug("API Request", &format!("{:?}", request));

        // Log the full request payload as JSON for detailed debugging
        match serde_json::to_string_pretty(&request) {
            Ok(json_payload) => {
                crate::utils::logging::debug("Full API Request Payload (JSON)", &json_payload);
            }
            Err(e) => {
                crate::utils::logging::error(
                    "API Request Serialization Error",
                    &format!("Failed to serialize request to JSON: {}", e),
                );
            }
        }

        let request_builder = self.http_client.post(API_URL).json(&request);

        // Race the request sending against cancellation
        let response = tokio::select! {
            biased;
            _ = token.cancelled() => {
                crate::utils::logging::debug("claude::complete", "Cancellation token triggered before sending request.");
                return Err(anyhow::anyhow!("Request cancelled"));
            }
            res = request_builder.send() => {
                res.context("Failed to send request to Claude API")?
            }
        };

        // Check for cancellation before processing status
        if token.is_cancelled() {
            crate::utils::logging::debug(
                "claude::complete",
                "Cancellation token triggered after sending request, before status check.",
            );
            return Err(anyhow::anyhow!("Request cancelled"));
        }

        if !response.status().is_success() {
            // Race reading the error text against cancellation
            let error_text = tokio::select! {
                biased;
                _ = token.cancelled() => {
                    crate::utils::logging::debug("claude::complete", "Cancellation token triggered while reading error response body.");
                    return Err(anyhow::anyhow!("Request cancelled"));
                }
                text_res = response.text() => {
                    text_res?
                }
            };
            return Err(anyhow::anyhow!("API error: {}", error_text));
        }

        // Race parsing the successful response against cancellation
        let claude_completion: ClaudeCompletionResponse = tokio::select! {
            biased;
            _ = token.cancelled() => {
                crate::utils::logging::debug("claude::complete", "Cancellation token triggered while parsing successful response body.");
                return Err(anyhow::anyhow!("Request cancelled"));
            }
            json_res = response.json() => {
                json_res.context("Failed to parse Claude API response")?
            }
        };

        // Convert Claude's response to our provider-agnostic format
        let completion = CompletionResponse {
            content: convert_claude_content(claude_completion.content),
            extra: claude_completion.extra,
        };

        Ok(completion)
    }
}
