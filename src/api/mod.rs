use anyhow::{Context, Result};
use async_stream::stream;
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::{self, header};
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

pub mod messages;
pub mod tools;

pub use messages::Message;
pub use tools::{Tool, ToolCall};

const API_URL: &str = "https://api.anthropic.com/v1/messages";

#[derive(Debug, Clone)]
pub struct Client {
    api_key: String,
    client: reqwest::Client,
    model: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompletionRequest {
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

#[derive(Debug, Serialize, Deserialize)]
pub struct CompletionResponse {
    id: String,
    content: Vec<ContentBlock>,
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
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

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Usage {
    #[serde(default)]
    input_tokens: usize,
    #[serde(default)]
    output_tokens: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StreamingCompletionResponse {
    #[serde(rename = "type")]
    response_type: String,
    #[serde(default)]
    message: Option<CompletionResponse>,
    #[serde(default)]
    delta: Option<Delta>,
    #[serde(default)]
    usage: Option<Usage>,
    // Additional fields that might be in newer API versions
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Delta {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    tool_use: Option<ToolUseDelta>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    stop_sequence: Option<String>,
    // Allow other fields for API flexibility
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ToolUseDelta {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    parameters: Option<serde_json::Value>,
    // Allow other fields for API flexibility
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

pub struct CompletionStream {
    response: Pin<Box<dyn Stream<Item = Result<String>> + Send>>,
}

impl Stream for CompletionStream {
    type Item = Result<String>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.response).poll_next(cx)
    }
}

impl Client {
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
            api_key: api_key.to_string(),
            client,
            model: "claude-3-7-sonnet-20250219".to_string(),
        }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }

    /// Complete a prompt with Claude
    pub async fn complete(
        &self,
        messages: Vec<Message>,
        system: Option<String>,
        tools: Option<Vec<Tool>>,
    ) -> Result<CompletionResponse> {
        let request = CompletionRequest {
            model: self.model.clone(),
            messages,
            max_tokens: 4000,
            system,
            tools,
            temperature: Some(0.7),
            top_p: None,
            top_k: None,
            stream: None,
        };

        crate::utils::logging::debug("API Request messages", &format!("{:?}", request.messages));

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

        let response = self
            .client
            .post(API_URL)
            .json(&request)
            .send()
            .await
            .context("Failed to send request to Claude API")?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!("API error: {}", error_text));
        }

        let completion: CompletionResponse = response
            .json()
            .await
            .context("Failed to parse Claude API response")?;

        Ok(completion)
    }
}

impl CompletionResponse {
    /// Check if the response contains any tool calls
    pub fn has_tool_calls(&self) -> bool {
        self.content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
    }

    /// Extract all tool calls from the response
    pub fn extract_tool_calls(&self) -> Vec<ToolCall> {
        self.content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::ToolUse {
                    id, name, input, ..
                } = block
                {
                    Some(ToolCall {
                        name: name.clone(),
                        parameters: input.clone(),
                        id: Some(id.clone()),
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    /// Extract all text content from the response
    pub fn extract_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::Text { text, .. } = block {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<String>>()
            .join("")
    }
}
