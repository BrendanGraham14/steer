use anyhow::{Result, anyhow};
use async_trait::async_trait;
use reqwest::{Client as HttpClient, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::api::Model;
use crate::api::error::ApiError;
use crate::api::messages::{ContentBlock, Message, MessageContent, MessageRole};
use crate::api::provider::{CompletionResponse, Provider};
use crate::api::tools::Tool;
use rand;

const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct GeminiClient {
    api_key: String,
    client: HttpClient,
}

impl GeminiClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: HttpClient::new(),
        }
    }
}

#[derive(Debug, Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "systemInstruction")]
    system_instruction: Option<GeminiSystemInstruction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiTool>>,
}

#[derive(Debug, Serialize)]
struct GeminiSystemInstruction {
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum GeminiPart {
    Text {
        text: String,
    },
    #[serde(rename = "functionCall")]
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    #[serde(rename = "functionResponse")]
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
    #[serde(rename = "executableCode")]
    ExecutableCode {
        #[serde(rename = "executableCode")]
        executable_code: GeminiExecutableCode,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: Value,
}

#[derive(Debug, Serialize)]
struct GeminiTool {
    #[serde(rename = "functionDeclarations")]
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Serialize)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: GeminiParameterSchema,
}

#[derive(Debug, Serialize)]
struct GeminiParameterSchema {
    #[serde(rename = "type")]
    schema_type: String, // Typically "object"
    properties: serde_json::Map<String, Value>,
    required: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
}

fn map_role(msg: &Message) -> &str {
    match msg.role {
        MessageRole::Assistant => "model",
        MessageRole::User => {
            // If a user message contains ANY ToolResult blocks, treat it as a function/tool response
            if let MessageContent::StructuredContent { ref content } = msg.content {
                // Check if any block is a ToolResult
                if content
                    .0
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
                {
                    return "function"; // Use "function" role for tool results
                }
            }
            "user"
        }
        // Treat our internal "tool" role as Gemini's "function" role
        MessageRole::Tool => "function",
        _ => "user", // Default to user for other roles
    }
}

fn convert_content_block_to_parts(
    block: ContentBlock,
    gemini_role: &str,
    message_for_logging: &Message, // Pass the whole message for logging context
) -> Vec<GeminiPart> {
    match block {
        ContentBlock::Text { text, .. } => {
            vec![GeminiPart::Text { text }]
        }
        ContentBlock::ToolUse {
            id: _, name, input, ..
        } => {
            // Assistant requests a tool call -> functionCall
            if gemini_role == "model" {
                vec![GeminiPart::FunctionCall {
                    function_call: GeminiFunctionCall { name, args: input },
                }]
            } else {
                warn!(target: "gemini::convert_content_block_to_parts", "Unexpected ToolUse block in non-model message (role: {}): {:?}", gemini_role, message_for_logging);
                vec![]
            }
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } => {
            // User provides tool result -> functionResponse
            if gemini_role == "function" {
                // Extract the actual result content (assuming text for now, needs enhancement for complex results)
                let result_value = content
                    .into_iter()
                    .find_map(|b| {
                        if let ContentBlock::Text { text, .. } = b {
                            // Attempt to parse text as JSON, otherwise treat as string
                            serde_json::from_str(&text).ok().or(Some(Value::String(text)))
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| {
                        // If no text or parsing fails, create a default Value (e.g., Null or error string)
                         warn!(target: "gemini::convert_content_block_to_parts", "Could not extract text or parse JSON from ToolResult content for tool_use_id '{}'.", tool_use_id);
                        serde_json::Value::Null // Use Null as a neutral default
                    });

                vec![GeminiPart::FunctionResponse {
                    function_response: GeminiFunctionResponse {
                        name: tool_use_id, // Use tool_use_id for the function name as required by Gemini
                        response: GeminiResponseContent {
                            content: result_value,
                        },
                    },
                }]
            } else {
                warn!(target: "gemini::convert_content_block_to_parts", "Unexpected ToolResult block in non-function message (role: {}): {:?}", gemini_role, message_for_logging);
                vec![]
            }
        }
    }
}

fn convert_messages(messages: Vec<Message>) -> Vec<GeminiContent> {
    messages
        .into_iter()
        .map(|msg| {
            // Determine role using helper
            let role = map_role(&msg);

            let parts = match msg.content {
                MessageContent::Text { ref content } => {
                    // Simple text message
                    vec![GeminiPart::Text {
                        text: content.clone(),
                    }]
                }
                MessageContent::StructuredContent { ref content } => {
                    // Convert structured content blocks to Gemini parts using helper
                    content
                        .0
                        .clone()
                        .into_iter()
                        .flat_map(|block| convert_content_block_to_parts(block, role, &msg))
                        .collect()
                }
            };

            GeminiContent {
                role: role.to_string(),
                parts,
            }
        })
        .collect()
}

fn simplify_property_schema(key: &str, tool_name: &str, property_value: &Value) -> Value {
    if let Some(prop_map_orig) = property_value.as_object() {
        let mut simplified_prop = prop_map_orig.clone();

        // Simplify 'type' field (handle arrays like ["string", "null"])
        if let Some(type_val) = simplified_prop.get_mut("type") {
            if let Some(type_array) = type_val.as_array() {
                if let Some(primary_type) = type_array
                    .iter()
                    .find_map(|v| if !v.is_null() { v.as_str() } else { None })
                {
                    *type_val = serde_json::Value::String(primary_type.to_string());
                } else {
                    warn!(target: "gemini::simplify_property_schema", "Could not determine primary type for property '{}' in tool '{}', defaulting to string.", key, tool_name);
                    *type_val = serde_json::Value::String("string".to_string());
                }
            } else if !type_val.is_string() {
                warn!(target: "gemini::simplify_property_schema", "Unexpected 'type' format for property '{}' in tool '{}': {:?}. Defaulting to string.", key, tool_name, type_val);
                *type_val = serde_json::Value::String("string".to_string());
            }
            // If it's already a simple string, do nothing.
        }

        // Fix integer format if necessary
        if simplified_prop.get("type") == Some(&serde_json::Value::String("integer".to_string())) {
            if let Some(format_val) = simplified_prop.get_mut("format") {
                if format_val.as_str() == Some("uint64") {
                    *format_val = serde_json::Value::String("int64".to_string());
                    // Optionally remove minimum if Gemini doesn't support it with int64
                    // simplified_prop.remove("minimum");
                }
            }
        }
        serde_json::Value::Object(simplified_prop)
    } else {
        warn!(target: "gemini::simplify_property_schema", "Property value for '{}' in tool '{}' is not an object: {:?}. Using original value.", key, tool_name, property_value);
        property_value.clone() // Return original if not an object
    }
}

fn convert_tools(tools: Vec<Tool>) -> Vec<GeminiTool> {
    let function_declarations = tools
        .into_iter()
        .map(|tool| {
            // Simplify properties schema for Gemini using the helper function
            let simplified_properties = tool
                .input_schema
                .properties
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        simplify_property_schema(key, &tool.name, value),
                    )
                })
                .collect();

            // Construct the parameters object using the specific struct
            let parameters = GeminiParameterSchema {
                schema_type: tool.input_schema.schema_type, // Use schema_type field (usually "object")
                properties: simplified_properties,          // Use simplified properties
                required: tool.input_schema.required,       // Use required field
            };

            GeminiFunctionDeclaration {
                name: tool.name,
                description: tool.description,
                parameters,
            }
        })
        .collect();

    vec![GeminiTool {
        function_declarations,
    }]
}

fn convert_response(response: GeminiResponse) -> Result<CompletionResponse> {
    if response.candidates.is_empty() {
        return Err(anyhow!("No candidates in Gemini response"));
    }

    let candidate = &response.candidates[0];
    let content = candidate
        .content
        .parts
        .iter()
        .map(|part| match part {
            GeminiPart::Text { text } => ContentBlock::Text {
                text: text.clone(),
            },
            GeminiPart::FunctionCall { function_call } => ContentBlock::ToolUse {
                id: format!("call_{}_{}", function_call.name, rand::random::<u32>()),
                name: function_call.name.clone(),
                input: function_call.args.clone(),
            },
            GeminiPart::FunctionResponse { function_response } => {
                // Convert FunctionResponse back to a generic structure if needed,
                // though typically the model response won't be a function *result*.
                // For now, maybe convert it to text or log a warning.
                warn!(target: "gemini::convert_response", "Unexpected FunctionResponse in model output: {:?}", function_response);
                // Fallback to representing it as text
                ContentBlock::Text {
                    text: format!("(Function Response: {})", function_response.name),
                }
            }
            GeminiPart::ExecutableCode { executable_code } => {
                info!(target: "gemini::convert_response", "Received ExecutableCode part ({}): {}. Converting to text.",
                     executable_code.language, executable_code.code);
                // Represent executable code as simple text for now
                info!(target: "gemini::convert_response", "Processing ExecutableCode part for response text conversion.");
                eprintln!("Received ExecutableCode part. See logs for more details.");
                ContentBlock::Text {
                    text: format!(
                        "```{}
{}
```",
                        executable_code.language.to_lowercase(),
                        executable_code.code
                    ),
                }
            }
        })
        .collect();

    Ok(CompletionResponse { content })
}

#[async_trait]
impl Provider for GeminiClient {
    fn name(&self) -> &'static str {
        "google"
    }

    async fn complete(
        &self,
        model: Model,
        messages: Vec<Message>,
        system: Option<String>,
        tools: Option<Vec<Tool>>,
        token: CancellationToken, // Keep token for potential future use
    ) -> Result<CompletionResponse, ApiError> {
        // <-- Use ApiError
        let model_name = model.as_ref();
        let url = format!(
            "{}/models/{}:generateContent?key={}",
            GEMINI_API_BASE, model_name, self.api_key
        );

        let gemini_contents = convert_messages(messages);

        let system_instruction = system.map(|instructions| GeminiSystemInstruction {
            parts: vec![GeminiPart::Text { text: instructions }],
        });

        let gemini_tools = tools.map(convert_tools);

        let request = GeminiRequest {
            contents: gemini_contents,
            system_instruction,
            tools: gemini_tools,
        };
        match serde_json::to_string_pretty(&request) {
            Ok(json_payload) => {
                debug!(target: "Full Gemini API Request Payload (JSON)", "{}", json_payload);
            }
            Err(e) => {
                error!(target: "Gemini API Request Serialization Error", "Failed to serialize request to JSON: {}", e);
            }
        }

        let response = tokio::select! {
            biased;
            _ = token.cancelled() => {
                debug!(target: "gemini::complete", "Cancellation token triggered before sending request.");
                return Err(ApiError::Cancelled{ provider: self.name().to_string()});
            }
            res = self.client.post(&url).json(&request).send() => {
                res.map_err(ApiError::Network)?
            }
        };
        let status = response.status();

        if status != StatusCode::OK {
            let error_text = response.text().await.map_err(ApiError::Network)?;
            error!(target: "Gemini API Error Response", "Status: {}, Body: {}", status, error_text);
            return Err(match status.as_u16() {
                401 | 403 => ApiError::AuthenticationFailed {
                    provider: self.name().to_string(),
                    details: error_text,
                },
                429 => ApiError::RateLimited {
                    provider: self.name().to_string(),
                    details: error_text,
                },
                400 | 404 => ApiError::InvalidRequest {
                    provider: self.name().to_string(),
                    details: error_text,
                }, // 404 might mean invalid model
                500..=599 => ApiError::ServerError {
                    provider: self.name().to_string(),
                    status_code: status.as_u16(),
                    details: error_text,
                },
                _ => ApiError::Unknown {
                    provider: self.name().to_string(),
                    details: error_text,
                },
            });
        }

        // Read the response body as text first to allow logging in case of JSON error
        let response_text = response.text().await.map_err(ApiError::Network)?;

        match serde_json::from_str::<GeminiResponse>(&response_text) {
            Ok(gemini_response) => {
                convert_response(gemini_response).map_err(|e| ApiError::ResponseParsingError {
                    provider: self.name().to_string(),
                    details: e.to_string(),
                })
            }
            Err(e) => {
                error!(target: "Gemini API JSON Parsing Error", "Failed to parse JSON: {}. Response body:\n{}", e, response_text);
                Err(ApiError::ResponseParsingError {
                    provider: self.name().to_string(),
                    details: format!("Error: {}, Body: {}", e, response_text),
                })
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiFunctionResponse {
    name: String,
    response: GeminiResponseContent,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiResponseContent {
    content: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiExecutableCode {
    language: String, // e.g., PYTHON
    code: String,
}
