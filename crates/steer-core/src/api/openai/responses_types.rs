use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
pub struct ResponsesFunctionTool {
    #[serde(rename = "type")]
    pub tool_type: String, // "function"
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: serde_json::Value,
    pub strict: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponsesToolChoice {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "required")]
    Required,
    Function {
        #[serde(rename = "type")]
        tool_type: String, // "function"
        name: String,
    },
}

/// Request body for the OpenAI "Responses" API (create response endpoint)
///
/// NOTE: This intentionally only includes the subset of parameters
/// we currently need inside Steer. The official specification
/// contains many more optional fields – we can extend this struct on
/// demand without breaking semver because all new fields will be
/// optional.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResponsesRequest {
    pub model: String,

    // Primary user prompt input(s).  The API allows strings, arrays of
    // mixed text / images, and file references.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<InputType>,

    /// Optional system / developer instructions injected into the context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,

    /// Previous response id for multi-turn conversations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,

    /// Temperature sampling parameter (0-2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Maximum output tokens (includes reasoning + visible output).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,

    /// Maximum total tool calls allowed in a single response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u32>,

    /// Allow built-in tools to run in parallel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,

    /// Persist the response for later retrieval.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,

    /// Stream the response via SSE.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,

    /// Select built-in or custom tools available to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ResponsesFunctionTool>>,

    /// Control how the model chooses tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ResponsesToolChoice>,

    /// Additional metadata for analytics / filtering.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,

    /// Service tier (auto, default, flex, priority).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>, // keep simple

    /// Advanced parameters for prompt configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<PromptConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextConfig>,

    /// Extra fields like `background`, `include`, `truncation`, `user`…
    #[serde(flatten)]
    pub extra: HashMap<String, ExtraValue>,
}

/// Top-level response object returned by the "Responses" API.
/// Only the fields required by Steer are deserialized – all other
/// data is captured in the `extra` map so we never lose information.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResponsesApiResponse {
    pub id: String,
    pub object: String,
    pub created_at: u64,
    pub status: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub incomplete_details: Option<ResponseIncompleteDetails>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Vec<ResponseOutputItem>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ResponseReasoning>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ResponseUsage>,

    #[serde(flatten)]
    pub extra: HashMap<String, ExtraValue>,
}

/// Response reasoning information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseReasoning {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Response usage information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
    pub input_tokens_details: InputTokensDetails,
    pub output_tokens_details: OutputTokensDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputTokensDetails {
    pub cached_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputTokensDetails {
    pub reasoning_tokens: u32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseOutputItem {
    Message {
        id: String,
        status: String,
        role: String,
        content: Vec<MessageContentPart>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        call_id: Option<String>,
        name: String,
        arguments: String,
        status: String,
    },
    #[serde(rename = "custom_tool_call")]
    CustomToolCall {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
        extra: HashMap<String, ExtraValue>,
    },
    #[serde(rename = "mcp_call")]
    McpCall {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        arguments: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
        extra: HashMap<String, ExtraValue>,
    },
    #[serde(rename = "mcp_approval_request")]
    McpApprovalRequest {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        arguments: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        server_label: Option<String>,
        #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
        extra: HashMap<String, ExtraValue>,
    },
    Reasoning {
        id: String,
        summary: Vec<ReasoningSummaryPart>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<Vec<ReasoningContentPart>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    #[serde(rename = "web_search_call")]
    WebSearchCall {
        id: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        query: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        results: Option<serde_json::Value>,
        #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
        extra: HashMap<String, ExtraValue>,
    },
    #[serde(rename = "file_search_call")]
    FileSearchCall {
        id: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        queries: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        results: Option<serde_json::Value>,
        #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
        extra: HashMap<String, ExtraValue>,
    },
    #[serde(rename = "code_interpreter_call")]
    CodeInterpreterCall {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        results: Option<serde_json::Value>,
        #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
        extra: HashMap<String, ExtraValue>,
    },
    #[serde(rename = "image_generation_call")]
    ImageGenerationCall {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
        #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
        extra: HashMap<String, ExtraValue>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContentPart {
    OutputText {
        text: String,
        #[serde(default)]
        annotations: Vec<Annotation>,
    },
    Refusal {
        refusal: String,
    },
    #[serde(other)]
    Other,
}

/// Parts that make up a reasoning summary block
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningSummaryPart {
    SummaryText {
        text: String,
    },
    #[serde(other)]
    Other,
}

impl ReasoningSummaryPart {
    pub(crate) fn text(&self) -> Option<&str> {
        match self {
            ReasoningSummaryPart::SummaryText { text } => Some(text.as_str()),
            ReasoningSummaryPart::Other => None,
        }
    }
}

/// Parts that make up reasoning content
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningContentPart {
    ReasoningText {
        text: String,
    },
    #[serde(other)]
    Other,
}

/// Input type for the responses API - can be text or array of messages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum InputType {
    Text(String),
    Messages(Vec<InputItem>),
}

/// Input item for structured message format
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum InputItem {
    Message {
        role: String,
        content: Vec<InputContentPart>,
    },
    FunctionResult {
        call_id: String,
        #[serde(rename = "output")]
        result: String,
    },
    FunctionCall {
        #[serde(rename = "type")]
        item_type: String, // "function_call"
        call_id: String,
        name: String,
        arguments: String,
    },
    FunctionCallOutput {
        #[serde(rename = "type")]
        item_type: String, // "function_call_output"
        call_id: String,
        output: String,
    },
}

/// Input content part for messages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputContentPart {
    #[serde(rename = "input_text")]
    InputText { text: String },
    #[serde(rename = "input_image")]
    InputImage {
        image_url: String,
        #[serde(default = "default_detail")]
        detail: String,
    },
    #[serde(rename = "input_file")]
    InputFile {
        #[serde(skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_url: Option<String>,
    },
    #[serde(rename = "output_text")]
    OutputText {
        text: String,
        #[serde(default)]
        annotations: Vec<Annotation>,
    },
    #[serde(rename = "refusal")]
    Refusal { refusal: String },
    #[serde(other)]
    Other,
}

fn default_detail() -> String {
    "auto".to_string()
}

/// Prompt configuration parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
}

/// Reasoning configuration parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ReasoningSummary>,
}

/// Reasoning effort levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

/// Reasoning summary verbosity
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningSummary {
    Auto,
    Concise,
    Detailed,
}

/// Text configuration parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<TextFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

/// Text format configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TextFormat {
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "json_schema")]
    JsonSchema { json_schema: serde_json::Value },
}

/// Response-level error details returned by the Responses API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "type")]
    pub error_type: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, ExtraValue>,
}

/// Details for incomplete responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseIncompleteDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, ExtraValue>,
}

/// Error envelope returned by non-success HTTP responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesHttpErrorEnvelope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
    #[serde(flatten)]
    pub extra: HashMap<String, ExtraValue>,
}

/// `error` SSE event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseErrorEvent {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "type")]
    pub event_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    #[serde(flatten)]
    pub extra: HashMap<String, ExtraValue>,
}

/// `response.failed` SSE event payload.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResponseFailedEvent {
    pub response: ResponsesApiResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "type")]
    pub event_type: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, ExtraValue>,
}

/// Annotation for content parts
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Annotation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_index: Option<u32>,
}

/// Extra value type for additional fields
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExtraValue {
    String(String),
    Number(f64),
    Bool(bool),
    Array(Vec<ExtraValue>),
    Object(HashMap<String, ExtraValue>),
    Null,
}
