use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{Client as HttpClient, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::api::error::{ApiError, SseParseError, StreamError};
use crate::api::provider::{CompletionResponse, CompletionStream, Provider, StreamChunk};
use crate::api::sse::parse_sse_stream;
use crate::app::SystemContext;
use crate::app::conversation::{
    AssistantContent, Message as AppMessage, ThoughtContent, ThoughtSignature, ToolResult,
    UserContent,
};
use crate::config::model::{ModelId, ModelParameters};
use steer_tools::ToolSchema;

const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

#[derive(Debug, Deserialize, Serialize, Clone)] // Added Serialize and Clone for potential future use
struct GeminiBlob {
    #[serde(rename = "mimeType")]
    mime_type: String,
    data: String, // Assuming base64 encoded data
}

#[derive(Debug, Deserialize, Serialize, Clone)] // Added Serialize and Clone
struct GeminiFileData {
    #[serde(rename = "mimeType")]
    mime_type: String,
    #[serde(rename = "fileUri")]
    file_uri: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)] // Added Serialize and Clone
struct GeminiCodeExecutionResult {
    outcome: String, // e.g., "OK", "ERROR"
                     // Potentially add output field later if needed
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "generationConfig")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Debug, Serialize, Default, Clone)]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "stopSequences")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "responseMimeType")]
    response_mime_type: Option<GeminiMimeType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "candidateCount")]
    candidate_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "topP")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "topK")]
    top_k: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "thinkingConfig")]
    thinking_config: Option<GeminiThinkingConfig>,
}

#[derive(Debug, Serialize, Default, Clone)]
struct GeminiThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "includeThoughts")]
    include_thoughts: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "thinkingBudget")]
    thinking_budget: Option<i32>,
}

#[expect(dead_code)]
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GeminiMimeType {
    MimeTypeUnspecified,
    TextPlain,
    ApplicationJson,
}

#[derive(Debug, Serialize)]
struct GeminiSystemInstruction {
    parts: Vec<GeminiRequestPart>,
}

#[derive(Debug, Serialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiRequestPart>,
}

// Enum for parts used ONLY in requests
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum GeminiRequestPart {
    Text {
        text: String,
    },
    #[serde(rename = "functionCall")]
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall, // Used for model turns in history
        #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    #[serde(rename = "functionResponse")]
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse, // Used for function/tool turns
    },
}

// Enum for parts received ONLY in responses
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum GeminiResponsePartData {
    Text {
        text: String,
    },
    #[serde(rename = "inlineData")]
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: GeminiBlob,
    },
    #[serde(rename = "functionCall")]
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    #[serde(rename = "fileData")]
    FileData {
        #[serde(rename = "fileData")]
        file_data: GeminiFileData,
    },
    #[serde(rename = "executableCode")]
    ExecutableCode {
        #[serde(rename = "executableCode")]
        executable_code: GeminiExecutableCode,
    },
    // Add other variants back here if needed
}

// 2. Change GeminiResponsePart to a struct
#[derive(Debug, Deserialize)]
struct GeminiResponsePart {
    #[serde(default)] // Defaults to false if missing
    thought: bool,
    #[serde(default, rename = "thoughtSignature", alias = "thought_signature")]
    thought_signature: Option<String>,

    #[serde(flatten)] // Look for data fields directly in this struct's JSON
    data: GeminiResponsePartData,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: Value,
}

#[derive(Debug, Serialize, PartialEq)]
struct GeminiTool {
    #[serde(rename = "functionDeclarations")]
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Serialize, PartialEq)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: GeminiParameterSchema,
}

#[derive(Debug, Serialize, PartialEq)]
struct GeminiParameterSchema {
    #[serde(rename = "type")]
    schema_type: String, // Typically "object"
    properties: serde_json::Map<String, Value>,
    required: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    #[serde(rename = "candidates")]
    #[serde(skip_serializing_if = "Option::is_none")]
    candidates: Option<Vec<GeminiCandidate>>,
    #[serde(rename = "promptFeedback")]
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_feedback: Option<GeminiPromptFeedback>,
    #[serde(rename = "usageMetadata")]
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: GeminiContentResponse,
    #[serde(rename = "finishReason")]
    #[serde(skip_serializing_if = "Option::is_none")]
    finish_reason: Option<GeminiFinishReason>,
    #[serde(rename = "safetyRatings")]
    #[serde(skip_serializing_if = "Option::is_none")]
    safety_ratings: Option<Vec<GeminiSafetyRating>>,
    #[serde(rename = "citationMetadata")]
    #[serde(skip_serializing_if = "Option::is_none")]
    citation_metadata: Option<GeminiCitationMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GeminiFinishReason {
    FinishReasonUnspecified,
    Stop,
    MaxTokens,
    Safety,
    Recitation,
    Other,
    #[serde(rename = "TOOL_CODE_ERROR")]
    ToolCodeError,
    #[serde(rename = "TOOL_EXECUTION_HALT")]
    ToolExecutionHalt,
    MalformedFunctionCall,
}

#[derive(Debug, Deserialize)]
struct GeminiPromptFeedback {
    #[serde(rename = "blockReason")]
    #[serde(skip_serializing_if = "Option::is_none")]
    block_reason: Option<GeminiBlockReason>,
    #[serde(rename = "safetyRatings")]
    #[serde(skip_serializing_if = "Option::is_none")]
    safety_ratings: Option<Vec<GeminiSafetyRating>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GeminiBlockReason {
    BlockReasonUnspecified,
    Safety,
    Other,
}

#[derive(Debug, Deserialize)]
#[expect(dead_code)]
struct GeminiSafetyRating {
    category: GeminiHarmCategory,
    probability: GeminiHarmProbability,
    #[serde(default)] // Default to false if missing
    blocked: bool,
}

#[derive(Debug, Deserialize, Serialize)] // Add Serialize for potential use in SafetySetting
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[expect(clippy::enum_variant_names)]
enum GeminiHarmCategory {
    HarmCategoryUnspecified,
    HarmCategoryDerogatory,
    HarmCategoryToxicity,
    HarmCategoryViolence,
    HarmCategorySexual,
    HarmCategoryMedical,
    HarmCategoryDangerous,
    HarmCategoryHarassment,
    HarmCategoryHateSpeech,
    HarmCategorySexuallyExplicit,
    HarmCategoryDangerousContent,
    HarmCategoryCivicIntegrity,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GeminiHarmProbability {
    HarmProbabilityUnspecified,
    Negligible,
    Low,
    Medium,
    High,
}

#[expect(dead_code)]
#[derive(Debug, Deserialize)]
struct GeminiCitationMetadata {
    #[serde(rename = "citationSources")]
    #[serde(skip_serializing_if = "Option::is_none")]
    citation_sources: Option<Vec<GeminiCitationSource>>,
}

#[expect(dead_code)]
#[derive(Debug, Deserialize)]
struct GeminiCitationSource {
    #[serde(rename = "startIndex")]
    #[serde(skip_serializing_if = "Option::is_none")]
    start_index: Option<i32>,
    #[serde(rename = "endIndex")]
    #[serde(skip_serializing_if = "Option::is_none")]
    end_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    license: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiUsageMetadata {
    #[serde(rename = "promptTokenCount")]
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_token_count: Option<i32>,
    #[serde(rename = "candidatesTokenCount")]
    #[serde(skip_serializing_if = "Option::is_none")]
    candidates_token_count: Option<i32>,
    #[serde(rename = "totalTokenCount")]
    #[serde(skip_serializing_if = "Option::is_none")]
    total_token_count: Option<i32>,
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

#[derive(Debug, Deserialize)]
#[expect(dead_code)]
struct GeminiContentResponse {
    role: String,
    parts: Vec<GeminiResponsePart>,
}

fn convert_messages(messages: Vec<AppMessage>) -> Vec<GeminiContent> {
    messages
        .into_iter()
        .filter_map(|msg| match &msg.data {
            crate::app::conversation::MessageData::User { content, .. } => {
                let parts: Vec<GeminiRequestPart> = content
                    .iter()
                    .map(|user_content| match user_content {
                        UserContent::Text { text } => {
                            GeminiRequestPart::Text { text: text.clone() }
                        }
                        UserContent::CommandExecution {
                            command,
                            stdout,
                            stderr,
                            exit_code,
                        } => GeminiRequestPart::Text {
                            text: UserContent::format_command_execution_as_xml(
                                command, stdout, stderr, *exit_code,
                            ),
                        },
                    })
                    .collect();

                // Only include the message if it has content after filtering
                if parts.is_empty() {
                    None
                } else {
                    Some(GeminiContent {
                        role: "user".to_string(),
                        parts,
                    })
                }
            }
            crate::app::conversation::MessageData::Assistant { content, .. } => {
                let parts: Vec<GeminiRequestPart> = content
                    .iter()
                    .filter_map(|assistant_content| match assistant_content {
                        AssistantContent::Text { text } => {
                            Some(GeminiRequestPart::Text { text: text.clone() })
                        }
                        AssistantContent::ToolCall {
                            tool_call,
                            thought_signature,
                        } => Some(GeminiRequestPart::FunctionCall {
                            function_call: GeminiFunctionCall {
                                name: tool_call.name.clone(),
                                args: tool_call.parameters.clone(),
                            },
                            thought_signature: thought_signature
                                .as_ref()
                                .map(|signature| signature.as_str().to_string()),
                        }),
                        AssistantContent::Thought { .. } => {
                            // Gemini doesn't send thought blocks in requests
                            None
                        }
                    })
                    .collect();

                // Always include assistant messages (they should always have content)
                Some(GeminiContent {
                    role: "model".to_string(),
                    parts,
                })
            }
            crate::app::conversation::MessageData::Tool {
                tool_use_id,
                result,
                ..
            } => {
                // Convert tool result to function response
                let result_value = match result {
                    ToolResult::Error(e) => Value::String(format!("Error: {e}")),
                    _ => {
                        // For all other variants, try to serialize as JSON
                        serde_json::to_value(result)
                            .unwrap_or_else(|_| Value::String(result.llm_format()))
                    }
                };

                let parts = vec![GeminiRequestPart::FunctionResponse {
                    function_response: GeminiFunctionResponse {
                        name: tool_use_id.clone(), // Use tool_use_id as function name
                        response: GeminiResponseContent {
                            content: result_value,
                        },
                    },
                }];

                Some(GeminiContent {
                    role: "function".to_string(),
                    parts,
                })
            }
        })
        .collect()
}

fn resolve_ref<'a>(root: &'a Value, schema: &'a Value) -> Option<&'a Value> {
    let reference = schema.get("$ref").and_then(|v| v.as_str())?;
    let path = reference.strip_prefix("#/")?;
    let mut current = root;
    for segment in path.split('/') {
        current = current.get(segment)?;
    }
    Some(current)
}

fn infer_type_from_enum(values: &[Value]) -> Option<String> {
    let mut has_string = false;
    let mut has_number = false;
    let mut has_bool = false;
    let mut has_object = false;
    let mut has_array = false;

    for value in values {
        match value {
            Value::String(_) => has_string = true,
            Value::Number(_) => has_number = true,
            Value::Bool(_) => has_bool = true,
            Value::Object(_) => has_object = true,
            Value::Array(_) => has_array = true,
            Value::Null => {}
        }
    }

    let kind_count = u8::from(has_string)
        + u8::from(has_number)
        + u8::from(has_bool)
        + u8::from(has_object)
        + u8::from(has_array);

    if kind_count != 1 {
        return None;
    }

    if has_string {
        Some("string".to_string())
    } else if has_number {
        Some("number".to_string())
    } else if has_bool {
        Some("boolean".to_string())
    } else if has_object {
        Some("object".to_string())
    } else if has_array {
        Some("array".to_string())
    } else {
        None
    }
}

fn normalize_type(value: &Value) -> Value {
    if let Some(type_str) = value.as_str() {
        return Value::String(type_str.to_string());
    }

    if let Some(type_array) = value.as_array()
        && let Some(primary_type) = type_array
            .iter()
            .find_map(|v| if v.is_null() { None } else { v.as_str() })
    {
        return Value::String(primary_type.to_string());
    }

    Value::String("string".to_string())
}

fn extract_enum_values(value: &Value) -> Vec<Value> {
    let Some(obj) = value.as_object() else {
        return Vec::new();
    };

    if let Some(enum_values) = obj.get("enum").and_then(|v| v.as_array()) {
        return enum_values
            .iter()
            .filter(|v| !v.is_null())
            .cloned()
            .collect();
    }

    if let Some(const_value) = obj.get("const") {
        if const_value.is_null() {
            return Vec::new();
        }
        return vec![const_value.clone()];
    }

    Vec::new()
}

fn merge_property(properties: &mut serde_json::Map<String, Value>, key: &str, value: &Value) {
    match properties.get_mut(key) {
        None => {
            properties.insert(key.to_string(), value.clone());
        }
        Some(existing) => {
            if existing == value {
                return;
            }

            let existing_values = extract_enum_values(existing);
            let incoming_values = extract_enum_values(value);
            if incoming_values.is_empty() && existing_values.is_empty() {
                return;
            }

            let mut combined = existing_values;
            for item in incoming_values {
                if !combined.contains(&item) {
                    combined.push(item);
                }
            }

            if combined.is_empty() {
                return;
            }

            if let Some(obj) = existing.as_object_mut() {
                obj.remove("const");
                obj.insert("enum".to_string(), Value::Array(combined.clone()));
                if !obj.contains_key("type")
                    && let Some(inferred) = infer_type_from_enum(&combined)
                {
                    obj.insert("type".to_string(), Value::String(inferred));
                }
            }
        }
    }
}

fn merge_union_schemas(root: &Value, variants: &[Value]) -> Value {
    let mut merged_props = serde_json::Map::new();
    let mut required_intersection: Option<std::collections::BTreeSet<String>> = None;
    let mut enum_values: Vec<Value> = Vec::new();
    let mut type_candidates: Vec<String> = Vec::new();

    for variant in variants {
        let sanitized = sanitize_for_gemini(root, variant);

        if let Some(schema_type) = sanitized.get("type").and_then(|v| v.as_str()) {
            type_candidates.push(schema_type.to_string());
        }

        if let Some(props) = sanitized.get("properties").and_then(|v| v.as_object()) {
            for (key, value) in props {
                merge_property(&mut merged_props, key, value);
            }
        }

        if let Some(req) = sanitized.get("required").and_then(|v| v.as_array()) {
            let req_set: std::collections::BTreeSet<String> = req
                .iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect();

            required_intersection = match required_intersection.take() {
                None => Some(req_set),
                Some(existing) => Some(
                    existing
                        .intersection(&req_set)
                        .cloned()
                        .collect::<std::collections::BTreeSet<String>>(),
                ),
            };
        }

        if let Some(values) = sanitized.get("enum").and_then(|v| v.as_array()) {
            for value in values {
                if value.is_null() {
                    continue;
                }
                if !enum_values.contains(value) {
                    enum_values.push(value.clone());
                }
            }
        }
    }

    let schema_type = if !merged_props.is_empty() {
        "object".to_string()
    } else if let Some(inferred) = infer_type_from_enum(&enum_values) {
        inferred
    } else if let Some(first) = type_candidates.first() {
        first.clone()
    } else {
        "string".to_string()
    };

    let mut merged = serde_json::Map::new();
    merged.insert("type".to_string(), Value::String(schema_type));

    if !merged_props.is_empty() {
        merged.insert("properties".to_string(), Value::Object(merged_props));
    }

    if let Some(required_set) = required_intersection
        && !required_set.is_empty()
    {
        merged.insert(
            "required".to_string(),
            Value::Array(
                required_set
                    .into_iter()
                    .map(Value::String)
                    .collect::<Vec<_>>(),
            ),
        );
    }

    if !enum_values.is_empty() {
        merged.insert("enum".to_string(), Value::Array(enum_values));
    }

    Value::Object(merged)
}

fn sanitize_for_gemini(root: &Value, schema: &Value) -> Value {
    if let Some(resolved) = resolve_ref(root, schema) {
        return sanitize_for_gemini(root, resolved);
    }

    let Some(obj) = schema.as_object() else {
        return schema.clone();
    };

    if let Some(union) = obj
        .get("oneOf")
        .or_else(|| obj.get("anyOf"))
        .or_else(|| obj.get("allOf"))
        .and_then(|v| v.as_array())
    {
        return merge_union_schemas(root, union);
    }

    let mut out = serde_json::Map::new();
    for (key, value) in obj {
        match key.as_str() {
            "$ref"
            | "$defs"
            | "oneOf"
            | "anyOf"
            | "allOf"
            | "const"
            | "additionalProperties"
            | "default"
            | "examples"
            | "title"
            | "pattern"
            | "minLength"
            | "maxLength"
            | "minimum"
            | "maximum"
            | "minItems"
            | "maxItems"
            | "uniqueItems"
            | "deprecated" => {
                continue;
            }
            "type" => {
                out.insert("type".to_string(), normalize_type(value));
            }
            "properties" => {
                if let Some(props) = value.as_object() {
                    let mut sanitized_props = serde_json::Map::new();
                    for (prop_key, prop_value) in props {
                        sanitized_props
                            .insert(prop_key.clone(), sanitize_for_gemini(root, prop_value));
                    }
                    out.insert("properties".to_string(), Value::Object(sanitized_props));
                }
            }
            "items" => {
                out.insert("items".to_string(), sanitize_for_gemini(root, value));
            }
            "enum" => {
                let values = value
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter(|v| !v.is_null())
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                out.insert("enum".to_string(), Value::Array(values));
            }
            _ => {
                out.insert(key.clone(), sanitize_for_gemini(root, value));
            }
        }
    }

    if let Some(const_value) = obj.get("const")
        && !const_value.is_null()
    {
        out.insert("enum".to_string(), Value::Array(vec![const_value.clone()]));
        if !out.contains_key("type")
            && let Some(inferred) = infer_type_from_enum(std::slice::from_ref(const_value))
        {
            out.insert("type".to_string(), Value::String(inferred));
        }
    }

    if out.get("type") == Some(&Value::String("object".to_string()))
        && !out.contains_key("properties")
    {
        out.insert(
            "properties".to_string(),
            Value::Object(serde_json::Map::new()),
        );
    }

    if !out.contains_key("type") {
        if out.contains_key("properties") {
            out.insert("type".to_string(), Value::String("object".to_string()));
        } else if let Some(enum_values) = out.get("enum").and_then(|v| v.as_array())
            && let Some(inferred) = infer_type_from_enum(enum_values)
        {
            out.insert("type".to_string(), Value::String(inferred));
        }
    }

    Value::Object(out)
}

fn simplify_property_schema(key: &str, tool_name: &str, property_value: &Value) -> Value {
    if let Some(prop_map_orig) = property_value.as_object() {
        let mut simplified_prop = prop_map_orig.clone();

        // Remove 'additionalProperties' as Gemini doesn't support it
        if simplified_prop.remove("additionalProperties").is_some() {
            debug!(target: "gemini::simplify_property_schema", "Removed 'additionalProperties' from property '{}' in tool '{}'", key, tool_name);
        }

        // Simplify 'type' field (handle arrays like ["string", "null"])
        if let Some(type_val) = simplified_prop.get_mut("type") {
            if let Some(type_array) = type_val.as_array() {
                if let Some(primary_type) = type_array
                    .iter()
                    .find_map(|v| if v.is_null() { None } else { v.as_str() })
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

        // For string types, Gemini only supports 'enum' and 'date-time' formats
        if simplified_prop.get("type") == Some(&serde_json::Value::String("string".to_string())) {
            let should_remove_format = simplified_prop
                .get("format")
                .and_then(|f| f.as_str())
                .is_some_and(|format_str| format_str != "enum" && format_str != "date-time");

            if should_remove_format {
                if let Some(format_val) = simplified_prop.remove("format") {
                    if let Some(format_str) = format_val.as_str() {
                        debug!(target: "gemini::simplify_property_schema", "Removed unsupported format '{}' from string property '{}' in tool '{}'", format_str, key, tool_name);
                    }
                }
            }

            // Also remove other string validation fields that might not be supported
            if simplified_prop.remove("minLength").is_some() {
                debug!(target: "gemini::simplify_property_schema", "Removed 'minLength' from string property '{}' in tool '{}'", key, tool_name);
            }
            if simplified_prop.remove("maxLength").is_some() {
                debug!(target: "gemini::simplify_property_schema", "Removed 'maxLength' from string property '{}' in tool '{}'", key, tool_name);
            }
            if simplified_prop.remove("pattern").is_some() {
                debug!(target: "gemini::simplify_property_schema", "Removed 'pattern' from string property '{}' in tool '{}'", key, tool_name);
            }
        }

        // Recursively simplify 'items' if this is an array type
        if simplified_prop.get("type") == Some(&serde_json::Value::String("array".to_string())) {
            if let Some(items_val) = simplified_prop.get_mut("items") {
                *items_val =
                    simplify_property_schema(&format!("{key}.items"), tool_name, items_val);
            }
        }

        // Recursively simplify nested 'properties' if this is an object type
        if simplified_prop.get("type") == Some(&serde_json::Value::String("object".to_string())) {
            if let Some(Value::Object(props)) = simplified_prop.get_mut("properties") {
                let simplified_nested_props: serde_json::Map<String, Value> = props
                    .iter()
                    .map(|(nested_key, nested_value)| {
                        (
                            nested_key.clone(),
                            simplify_property_schema(
                                &format!("{key}.{nested_key}"),
                                tool_name,
                                nested_value,
                            ),
                        )
                    })
                    .collect();
                *props = simplified_nested_props;
            }
        }

        serde_json::Value::Object(simplified_prop)
    } else {
        warn!(target: "gemini::simplify_property_schema", "Property value for '{}' in tool '{}' is not an object: {:?}. Using original value.", key, tool_name, property_value);
        property_value.clone() // Return original if not an object
    }
}

fn convert_tools(tools: Vec<ToolSchema>) -> Vec<GeminiTool> {
    let function_declarations = tools
        .into_iter()
        .map(|tool| {
            let root_schema = tool.input_schema.as_value();
            let summary = tool.input_schema.summary();
            let schema_type = if summary.schema_type.is_empty() {
                "object".to_string()
            } else {
                summary.schema_type
            };

            // Simplify properties schema for Gemini using the helper function
            let simplified_properties = summary
                .properties
                .iter()
                .map(|(key, value)| {
                    let sanitized = sanitize_for_gemini(root_schema, value);
                    (
                        key.clone(),
                        simplify_property_schema(key, &tool.name, &sanitized),
                    )
                })
                .collect();

            // Construct the parameters object using the specific struct
            let parameters = GeminiParameterSchema {
                schema_type,                       // Use schema_type field (usually "object")
                properties: simplified_properties, // Use simplified properties
                required: summary.required,        // Use required field
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

fn convert_response(response: GeminiResponse) -> Result<CompletionResponse, ApiError> {
    // Log prompt feedback if present
    if let Some(feedback) = &response.prompt_feedback {
        if let Some(reason) = &feedback.block_reason {
            let details = format!(
                "Prompt blocked due to {:?}. Safety ratings: {:?}",
                reason, feedback.safety_ratings
            );
            warn!(target: "gemini::convert_response", "{}", details);
            // Return the specific RequestBlocked error
            return Err(ApiError::RequestBlocked {
                provider: "google".to_string(), // Assuming "google" is the provider name
                details,
            });
        }
    }

    // Check candidates *after* checking for prompt blocking
    let candidates = if let Some(cands) = response.candidates {
        if cands.is_empty() {
            // If it was blocked, the previous check should have caught it.
            // So, this means no candidates were generated for other reasons.
            warn!(target: "gemini::convert_response", "No candidates received, and prompt was not blocked.");
            // Use NoChoices error here
            return Err(ApiError::NoChoices {
                provider: "google".to_string(),
            });
        }
        cands // Return the non-empty vector
    } else {
        warn!(target: "gemini::convert_response", "No candidates field in Gemini response.");
        // Use NoChoices error here as well
        return Err(ApiError::NoChoices {
            provider: "google".to_string(),
        });
    };

    // For simplicity, still taking the first candidate. Multi-candidate handling could be added.
    // Access candidates safely since we've checked it's not None or empty.
    let candidate = &candidates[0];

    // Log finish reason and safety ratings if present
    if let Some(reason) = &candidate.finish_reason {
        match reason {
            GeminiFinishReason::Stop => { /* Normal completion */ }
            GeminiFinishReason::MaxTokens => {
                warn!(target: "gemini::convert_response", "Response stopped due to MaxTokens limit.");
            }
            GeminiFinishReason::Safety => {
                warn!(target: "gemini::convert_response", "Response stopped due to safety settings. Ratings: {:?}", candidate.safety_ratings);
                // Consider returning an error or modifying the response based on safety ratings
            }
            GeminiFinishReason::Recitation => {
                warn!(target: "gemini::convert_response", "Response stopped due to potential recitation. Citations: {:?}", candidate.citation_metadata);
            }
            GeminiFinishReason::MalformedFunctionCall => {
                warn!(target: "gemini::convert_response", "Response stopped due to malformed function call.");
            }
            _ => {
                info!(target: "gemini::convert_response", "Response finished with reason: {:?}", reason);
            }
        }
    }

    // Log usage metadata if present
    if let Some(usage) = &response.usage_metadata {
        debug!(target: "gemini::convert_response", "Usage - Prompt Tokens: {:?}, Candidates Tokens: {:?}, Total Tokens: {:?}",
               usage.prompt_token_count, usage.candidates_token_count, usage.total_token_count);
    }

    let content: Vec<AssistantContent> = candidate
        .content // GeminiContentResponse
        .parts   // Vec<GeminiResponsePart> (struct)
        .iter()
        .filter_map(|part| { // part is &GeminiResponsePart (struct)
            // Check if this is a thought part first
            if part.thought {
                debug!(target: "gemini::convert_response", "Received thought part: {:?}", part);
                // For thought parts, extract text content and create a Thought block
                if let GeminiResponsePartData::Text { text } = &part.data {
                    Some(AssistantContent::Thought {
                        thought: ThoughtContent::Simple {
                            text: text.clone(),
                        },
                    })
                } else {
                    warn!(target: "gemini::convert_response", "Thought part contains non-text data: {:?}", part.data);
                    None
                }
            } else {
                // Regular (non-thought) content processing
                match &part.data {
                    GeminiResponsePartData::Text { text } => Some(AssistantContent::Text {
                        text: text.clone(),
                    }),
                    GeminiResponsePartData::InlineData { inline_data } => {
                        warn!(target: "gemini::convert_response", "Received InlineData part (MIME type: {}). Converting to placeholder text.", inline_data.mime_type);
                        Some(AssistantContent::Text { text: format!("[Inline Data: {}]", inline_data.mime_type) })
                    }
                    GeminiResponsePartData::FunctionCall { function_call } => {
                        Some(AssistantContent::ToolCall {
                            tool_call: steer_tools::ToolCall {
                                id: uuid::Uuid::new_v4().to_string(), // Generate a synthetic ID
                                name: function_call.name.clone(),
                                parameters: function_call.args.clone(),
                            },
                            thought_signature: part
                                .thought_signature
                                .clone()
                                .map(ThoughtSignature::new),
                        })
                    }
                    GeminiResponsePartData::FileData { file_data } => {
                        warn!(target: "gemini::convert_response", "Received FileData part (URI: {}). Converting to placeholder text.", file_data.file_uri);
                        Some(AssistantContent::Text { text: format!("[File Data: {}]", file_data.file_uri) })
                    }
                     GeminiResponsePartData::ExecutableCode { executable_code } => {
                         info!(target: "gemini::convert_response", "Received ExecutableCode part ({}). Converting to text.",
                              executable_code.language);
                         Some(AssistantContent::Text {
                             text: format!(
                                 "```{}
{}
```",
                                 executable_code.language.to_lowercase(),
                                 executable_code.code
                             ),
                         })
                     }
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
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        _call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        let model_name = &model_id.id; // Use the model ID string
        let url = format!(
            "{}/models/{}:generateContent?key={}",
            GEMINI_API_BASE, model_name, self.api_key
        );

        let gemini_contents = convert_messages(messages);

        let system_instruction = system
            .and_then(|context| context.render())
            .map(|instructions| GeminiSystemInstruction {
                parts: vec![GeminiRequestPart::Text { text: instructions }],
            });

        let gemini_tools = tools.map(convert_tools);

        // Derive generation config from call options, respecting model/catalog settings
        let (temperature, top_p, max_output_tokens) = {
            let opts = _call_options.as_ref();
            (
                opts.and_then(|o| o.temperature).or(Some(1.0)),
                opts.and_then(|o| o.top_p).or(Some(0.95)),
                opts.and_then(|o| o.max_tokens)
                    .map(|v| v as i32)
                    .or(Some(65536)),
            )
        };
        let thinking_config = _call_options
            .as_ref()
            .and_then(|o| o.thinking_config)
            .and_then(|tc| {
                if tc.enabled {
                    Some(GeminiThinkingConfig {
                        include_thoughts: tc.include_thoughts,
                        thinking_budget: tc.budget_tokens.map(|v| v as i32),
                    })
                } else {
                    None
                }
            });

        let request = GeminiRequest {
            contents: gemini_contents,
            system_instruction,
            tools: gemini_tools,
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens,
                temperature,
                top_p,
                thinking_config,
                ..Default::default()
            }),
        };

        let response = tokio::select! {
            biased;
            () = token.cancelled() => {
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
                400 | 404 => {
                    error!(target: "Gemini API Error Response", "Status: {}, Body: {}, Request: {}", status, error_text, serde_json::to_string_pretty(&request).unwrap_or_else(|_| "Failed to serialize request".to_string()));
                    ApiError::InvalidRequest {
                        provider: self.name().to_string(),
                        details: error_text,
                    }
                } // 404 might mean invalid model
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
                    details: format!("Status: {status}, Error: {e}, Body: {response_text}"),
                })
            }
        }
    }

    async fn stream_complete(
        &self,
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        _call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionStream, ApiError> {
        let model_name = &model_id.id;
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            GEMINI_API_BASE, model_name, self.api_key
        );

        let gemini_contents = convert_messages(messages);

        let system_instruction = system
            .and_then(|context| context.render())
            .map(|instructions| GeminiSystemInstruction {
                parts: vec![GeminiRequestPart::Text { text: instructions }],
            });

        let gemini_tools = tools.map(convert_tools);

        let (temperature, top_p, max_output_tokens) = {
            let opts = _call_options.as_ref();
            (
                opts.and_then(|o| o.temperature).or(Some(1.0)),
                opts.and_then(|o| o.top_p).or(Some(0.95)),
                opts.and_then(|o| o.max_tokens)
                    .map(|v| v as i32)
                    .or(Some(65536)),
            )
        };
        let thinking_config = _call_options
            .as_ref()
            .and_then(|o| o.thinking_config)
            .and_then(|tc| {
                if tc.enabled {
                    Some(GeminiThinkingConfig {
                        include_thoughts: tc.include_thoughts,
                        thinking_budget: tc.budget_tokens.map(|v| v as i32),
                    })
                } else {
                    None
                }
            });

        let request = GeminiRequest {
            contents: gemini_contents,
            system_instruction,
            tools: gemini_tools,
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens,
                temperature,
                top_p,
                thinking_config,
                ..Default::default()
            }),
        };

        let response = tokio::select! {
            biased;
            () = token.cancelled() => {
                return Err(ApiError::Cancelled{ provider: self.name().to_string()});
            }
            res = self.client.post(&url).json(&request).send() => {
                res.map_err(ApiError::Network)?
            }
        };

        let status = response.status();
        if status != StatusCode::OK {
            let error_text = response.text().await.map_err(ApiError::Network)?;
            error!(target: "gemini::stream", "API error - Status: {}, Body: {}", status, error_text);
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
                },
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

        let byte_stream = response.bytes_stream();
        let sse_stream = parse_sse_stream(byte_stream);

        Ok(Box::pin(Self::convert_gemini_stream(sse_stream, token)))
    }
}

impl GeminiClient {
    fn convert_gemini_stream(
        mut sse_stream: impl futures::Stream<Item = Result<crate::api::sse::SseEvent, SseParseError>>
        + Unpin
        + Send
        + 'static,
        token: CancellationToken,
    ) -> impl futures::Stream<Item = StreamChunk> + Send + 'static {
        async_stream::stream! {
            let mut content: Vec<AssistantContent> = Vec::new();
            loop {
                if token.is_cancelled() {
                    yield StreamChunk::Error(StreamError::Cancelled);
                    break;
                }

                let event_result = tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        yield StreamChunk::Error(StreamError::Cancelled);
                        break;
                    }
                    event = sse_stream.next() => event
                };

                let Some(event_result) = event_result else {
                    let content = std::mem::take(&mut content);
                    yield StreamChunk::MessageComplete(CompletionResponse { content });
                    break;
                };

                let event = match event_result {
                    Ok(e) => e,
                    Err(e) => {
                        yield StreamChunk::Error(StreamError::SseParse(e));
                        break;
                    }
                };

                let chunk: GeminiResponse = match serde_json::from_str(&event.data) {
                    Ok(c) => c,
                    Err(e) => {
                        debug!(target: "gemini::stream", "Failed to parse chunk: {} data: {}", e, event.data);
                        continue;
                    }
                };

                if let Some(candidates) = chunk.candidates {
                    for candidate in candidates {
                        for part in candidate.content.parts {
                            let GeminiResponsePart {
                                thought,
                                thought_signature,
                                data,
                            } = part;
                            let thought_signature =
                                thought_signature.map(ThoughtSignature::new);

                            if thought {
                                if let GeminiResponsePartData::Text { text } = data {
                                    match content.last_mut() {
                                        Some(AssistantContent::Thought {
                                            thought: ThoughtContent::Simple { text: buf },
                                        }) => buf.push_str(&text),
                                        _ => {
                                            content.push(AssistantContent::Thought {
                                                thought: ThoughtContent::Simple { text: text.clone() },
                                            });
                                        }
                                    }
                                    yield StreamChunk::ThinkingDelta(text);
                                }
                            } else {
                                match data {
                                    GeminiResponsePartData::Text { text } => {
                                        match content.last_mut() {
                                            Some(AssistantContent::Text { text: buf }) => buf.push_str(&text),
                                            _ => {
                                                content.push(AssistantContent::Text { text: text.clone() });
                                            }
                                        }
                                        yield StreamChunk::TextDelta(text);
                                    }
                                    GeminiResponsePartData::FunctionCall { function_call } => {
                                        let id = uuid::Uuid::new_v4().to_string();
                                        content.push(AssistantContent::ToolCall {
                                            tool_call: steer_tools::ToolCall {
                                                id: id.clone(),
                                                name: function_call.name.clone(),
                                                parameters: function_call.args.clone(),
                                            },
                                            thought_signature,
                                        });
                                        yield StreamChunk::ToolUseStart {
                                            id: id.clone(),
                                            name: function_call.name,
                                        };
                                        yield StreamChunk::ToolUseInputDelta {
                                            id,
                                            delta: function_call.args.to_string(),
                                        };
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simplify_property_schema_removes_additional_properties() {
        let property_value = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            },
            "additionalProperties": false
        });

        let expected = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            }
        });

        let result = simplify_property_schema("testProp", "testTool", &property_value);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_simplify_property_schema_removes_unsupported_string_formats() {
        let property_value = json!({
            "type": "string",
            "format": "uri",
            "minLength": 1,
            "maxLength": 100,
            "pattern": "^https://"
        });

        let expected = json!({
            "type": "string"
        });

        let result = simplify_property_schema("urlProp", "testTool", &property_value);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_simplify_property_schema_keeps_supported_string_formats() {
        let property_value = json!({
            "type": "string",
            "format": "date-time"
        });

        let expected = json!({
            "type": "string",
            "format": "date-time"
        });

        let result = simplify_property_schema("dateProp", "testTool", &property_value);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_simplify_property_schema_handles_array_types() {
        let property_value = json!({
            "type": ["string", "null"],
            "format": "email"
        });

        let expected = json!({
            "type": "string"
        });

        let result = simplify_property_schema("emailProp", "testTool", &property_value);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_simplify_property_schema_recursively_handles_array_items() {
        let property_value = json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "format": "uri"
                    }
                },
                "additionalProperties": false
            }
        });

        let expected = json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string"
                    }
                }
            }
        });

        let result = simplify_property_schema("linksProp", "testTool", &property_value);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_simplify_property_schema_recursively_handles_nested_objects() {
        let property_value = json!({
            "type": "object",
            "properties": {
                "nested": {
                    "type": "object",
                    "properties": {
                        "field": {
                            "type": "string",
                            "format": "hostname"
                        }
                    },
                    "additionalProperties": true
                }
            },
            "additionalProperties": false
        });

        let expected = json!({
            "type": "object",
            "properties": {
                "nested": {
                    "type": "object",
                    "properties": {
                        "field": {
                            "type": "string"
                        }
                    }
                }
            }
        });

        let result = simplify_property_schema("complexProp", "testTool", &property_value);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_simplify_property_schema_fixes_uint64_format() {
        let property_value = json!({
            "type": "integer",
            "format": "uint64"
        });

        let expected = json!({
            "type": "integer",
            "format": "int64"
        });

        let result = simplify_property_schema("idProp", "testTool", &property_value);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_convert_tools_integration() {
        use steer_tools::{InputSchema, ToolSchema};

        let tool = ToolSchema {
            name: "create_issue".to_string(),
            display_name: "Create Issue".to_string(),
            description: "Create an issue".to_string(),
            input_schema: InputSchema::object(
                {
                    let mut props = serde_json::Map::new();
                    props.insert(
                        "title".to_string(),
                        json!({
                            "type": "string",
                            "minLength": 1
                        }),
                    );
                    props.insert(
                        "links".to_string(),
                        json!({
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "url": {
                                        "type": "string",
                                        "format": "uri"
                                    }
                                },
                                "additionalProperties": false
                            }
                        }),
                    );
                    props
                },
                vec!["title".to_string()],
            ),
        };

        let expected_tools = vec![GeminiTool {
            function_declarations: vec![GeminiFunctionDeclaration {
                name: "create_issue".to_string(),
                description: "Create an issue".to_string(),
                parameters: GeminiParameterSchema {
                    schema_type: "object".to_string(),
                    properties: {
                        let mut props = serde_json::Map::new();
                        props.insert(
                            "title".to_string(),
                            json!({
                                "type": "string"
                            }),
                        );
                        props.insert(
                            "links".to_string(),
                            json!({
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "url": {
                                            "type": "string"
                                        }
                                    }
                                }
                            }),
                        );
                        props
                    },
                    required: vec!["title".to_string()],
                },
            }],
        }];

        let result = convert_tools(vec![tool]);
        assert_eq!(result, expected_tools);
    }

    #[tokio::test]
    async fn test_convert_gemini_stream_text_deltas() {
        use crate::api::provider::StreamChunk;
        use crate::api::sse::SseEvent;
        use futures::StreamExt;
        use futures::stream;
        use std::pin::pin;
        use tokio_util::sync::CancellationToken;

        let events = vec![
            Ok(SseEvent {
                event_type: None,
                data: r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello"}]}}]}"#
                    .to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: None,
                data:
                    r#"{"candidates":[{"content":{"role":"model","parts":[{"text":" world"}]}}]}"#
                        .to_string(),
                id: None,
            }),
        ];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(GeminiClient::convert_gemini_stream(sse_stream, token));

        let first_delta = stream.next().await.unwrap();
        assert!(matches!(first_delta, StreamChunk::TextDelta(ref t) if t == "Hello"));

        let second_delta = stream.next().await.unwrap();
        assert!(matches!(second_delta, StreamChunk::TextDelta(ref t) if t == " world"));

        let complete = stream.next().await.unwrap();
        assert!(matches!(complete, StreamChunk::MessageComplete(_)));
    }

    #[tokio::test]
    async fn test_convert_gemini_stream_with_thinking() {
        use crate::api::provider::StreamChunk;
        use crate::api::sse::SseEvent;
        use futures::StreamExt;
        use futures::stream;
        use std::pin::pin;
        use tokio_util::sync::CancellationToken;

        let events = vec![
            Ok(SseEvent {
                event_type: None,
                data: r#"{"candidates":[{"content":{"role":"model","parts":[{"thought":true,"text":"Let me think..."}]}}]}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: None,
                data: r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"The answer"}]}}]}"#.to_string(),
                id: None,
            }),
        ];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(GeminiClient::convert_gemini_stream(sse_stream, token));

        let thinking_delta = stream.next().await.unwrap();
        assert!(
            matches!(thinking_delta, StreamChunk::ThinkingDelta(ref t) if t == "Let me think...")
        );

        let text_delta = stream.next().await.unwrap();
        assert!(matches!(text_delta, StreamChunk::TextDelta(ref t) if t == "The answer"));

        let complete = stream.next().await.unwrap();
        if let StreamChunk::MessageComplete(response) = complete {
            assert_eq!(response.content.len(), 2);
            assert!(matches!(
                &response.content[0],
                AssistantContent::Thought { .. }
            ));
            assert!(matches!(
                &response.content[1],
                AssistantContent::Text { .. }
            ));
        } else {
            panic!("Expected MessageComplete");
        }
    }

    #[tokio::test]
    async fn test_convert_gemini_stream_with_function_call() {
        use crate::api::provider::StreamChunk;
        use crate::api::sse::SseEvent;
        use futures::StreamExt;
        use futures::stream;
        use std::pin::pin;
        use tokio_util::sync::CancellationToken;

        let events = vec![
            Ok(SseEvent {
                event_type: None,
                data: r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"get_weather","args":{"city":"NYC"}},"thoughtSignature":"sig_123"}]}}]}"#.to_string(),
                id: None,
            }),
        ];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(GeminiClient::convert_gemini_stream(sse_stream, token));

        let tool_start = stream.next().await.unwrap();
        assert!(
            matches!(tool_start, StreamChunk::ToolUseStart { ref name, .. } if name == "get_weather")
        );

        let tool_input = stream.next().await.unwrap();
        assert!(matches!(tool_input, StreamChunk::ToolUseInputDelta { .. }));

        let complete = stream.next().await.unwrap();
        if let StreamChunk::MessageComplete(response) = complete {
            assert_eq!(response.content.len(), 1);
            if let AssistantContent::ToolCall {
                tool_call,
                thought_signature,
            } = &response.content[0]
            {
                assert_eq!(tool_call.name, "get_weather");
                assert_eq!(
                    thought_signature.as_ref().map(|sig| sig.as_str()),
                    Some("sig_123")
                );
            } else {
                panic!("Expected ToolCall");
            }
        } else {
            panic!("Expected MessageComplete");
        }
    }

    #[tokio::test]
    async fn test_convert_gemini_stream_cancellation() {
        use crate::api::error::StreamError;
        use crate::api::provider::StreamChunk;
        use crate::api::sse::SseEvent;
        use futures::StreamExt;
        use futures::stream;
        use std::pin::pin;
        use tokio_util::sync::CancellationToken;

        let events = vec![Ok(SseEvent {
            event_type: None,
            data: r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello"}]}}]}"#
                .to_string(),
            id: None,
        })];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        token.cancel();

        let mut stream = pin!(GeminiClient::convert_gemini_stream(sse_stream, token));

        let cancelled = stream.next().await.unwrap();
        assert!(matches!(
            cancelled,
            StreamChunk::Error(StreamError::Cancelled)
        ));
    }

    #[tokio::test]
    #[ignore = "Requires GOOGLE_API_KEY environment variable"]
    async fn test_stream_complete_real_api() {
        use crate::api::Provider;
        use crate::api::provider::StreamChunk;
        use crate::app::conversation::{Message, MessageData, UserContent};
        use futures::StreamExt;
        use tokio_util::sync::CancellationToken;

        dotenvy::dotenv().ok();
        let api_key = std::env::var("GOOGLE_API_KEY").expect("GOOGLE_API_KEY must be set");
        let client = GeminiClient::new(api_key);

        let message = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Say exactly: Hello".to_string(),
                }],
            },
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            id: "test-msg".to_string(),
            parent_message_id: None,
        };

        let model_id = ModelId::new(
            crate::config::provider::google(),
            "gemini-2.5-flash-preview-04-17",
        );
        let token = CancellationToken::new();

        let mut stream = client
            .stream_complete(&model_id, vec![message], None, None, None, token)
            .await
            .expect("stream_complete should succeed");

        let mut got_text_delta = false;
        let mut got_message_complete = false;
        let mut accumulated_text = String::new();

        while let Some(chunk) = stream.next().await {
            match chunk {
                StreamChunk::TextDelta(text) => {
                    got_text_delta = true;
                    accumulated_text.push_str(&text);
                }
                StreamChunk::MessageComplete(response) => {
                    got_message_complete = true;
                    assert!(!response.content.is_empty());
                }
                StreamChunk::Error(e) => panic!("Unexpected error: {e:?}"),
                _ => {}
            }
        }

        assert!(got_text_delta, "Should receive at least one TextDelta");
        assert!(
            got_message_complete,
            "Should receive MessageComplete at the end"
        );
        assert!(
            accumulated_text.to_lowercase().contains("hello"),
            "Response should contain 'hello', got: {accumulated_text}"
        );
    }
}
