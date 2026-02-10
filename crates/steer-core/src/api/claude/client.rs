use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{self, header};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use strum_macros::Display;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::api::error::StreamError;
use crate::api::provider::{CompletionStream, StreamChunk};
use crate::api::sse::parse_sse_stream;
use crate::api::{CompletionResponse, Provider, error::ApiError};
use crate::app::SystemContext;
use crate::app::conversation::{
    AssistantContent, Message as AppMessage, ThoughtContent, ToolResult, UserContent,
};
use crate::auth::{
    AnthropicAuth, AuthErrorAction, AuthErrorContext, AuthHeaderContext, InstructionPolicy,
    RequestKind,
};
use crate::auth::{ModelId as AuthModelId, ProviderId as AuthProviderId};
use crate::config::model::{ModelId, ModelParameters};
use steer_tools::{InputSchema, ToolCall, ToolSchema};

const API_URL: &str = "https://api.anthropic.com/v1/messages";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Display)]
pub enum ClaudeMessageRole {
    #[serde(rename = "user")]
    #[strum(serialize = "user")]
    User,
    #[serde(rename = "assistant")]
    #[strum(serialize = "assistant")]
    Assistant,
    #[serde(rename = "tool")]
    #[strum(serialize = "tool")]
    Tool,
}

/// Represents a message to be sent to the Claude API
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaudeMessage {
    pub role: ClaudeMessageRole,
    #[serde(flatten)]
    pub content: ClaudeMessageContent,
    #[serde(skip_serializing)]
    pub id: Option<String>,
}

/// Content types for Claude API messages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ClaudeMessageContent {
    /// Simple text content
    Text { content: String },
    /// Structured content for tool results or other special content
    StructuredContent { content: ClaudeStructuredContent },
}

/// Represents structured content blocks for messages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct ClaudeStructuredContent(pub Vec<ClaudeContentBlock>);

#[derive(Clone)]
enum AuthMode {
    ApiKey(String),
    Directive(AnthropicAuth),
}

#[derive(Clone)]
pub struct AnthropicClient {
    http_client: reqwest::Client,
    auth: AuthMode,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
enum ThinkingType {
    #[default]
    Enabled,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Thinking {
    #[serde(rename = "type", default)]
    thinking_type: ThinkingType,
    budget_tokens: u32,
}

#[derive(Debug, Serialize, Clone)]
struct SystemContentBlock {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(untagged)]
enum System {
    // Structured system prompt represented as a list of content blocks
    Content(Vec<SystemContentBlock>),
}

#[derive(Debug, Serialize)]
struct CompletionRequest {
    model: String,
    messages: Vec<ClaudeMessage>,
    max_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<System>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ClaudeTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<Thinking>,
}

#[derive(Debug, Serialize, Clone)]
struct ClaudeTool {
    name: String,
    description: String,
    input_schema: InputSchema,
}

impl From<ToolSchema> for ClaudeTool {
    fn from(tool: ToolSchema) -> Self {
        let tool = adapt_tool_schema_for_claude(tool);
        Self {
            name: tool.name,
            description: tool.description,
            input_schema: tool.input_schema,
        }
    }
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
    usage: ClaudeUsage,
    // Allow other fields for API flexibility
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

fn adapt_tool_schema_for_claude(tool: ToolSchema) -> ToolSchema {
    let root_schema = tool.input_schema.as_value();
    let sanitized = sanitize_for_claude(root_schema, root_schema);
    ToolSchema {
        input_schema: InputSchema::new(sanitized),
        ..tool
    }
}

fn decode_pointer_segment(segment: &str) -> std::borrow::Cow<'_, str> {
    if !segment.contains('~') {
        return std::borrow::Cow::Borrowed(segment);
    }
    std::borrow::Cow::Owned(segment.replace("~1", "/").replace("~0", "~"))
}

fn resolve_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let path = reference.strip_prefix("#/")?;
    let mut current = root;
    for segment in path.split('/') {
        let decoded = decode_pointer_segment(segment);
        current = current.get(decoded.as_ref())?;
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

fn merge_union_schemas(
    root: &Value,
    variants: &[Value],
    seen_refs: &mut std::collections::HashSet<String>,
) -> Value {
    let mut merged_props = serde_json::Map::new();
    let mut required_intersection: Option<std::collections::BTreeSet<String>> = None;
    let mut enum_values: Vec<Value> = Vec::new();
    let mut type_candidates: Vec<String> = Vec::new();

    for variant in variants {
        let sanitized = sanitize_for_claude_inner(root, variant, seen_refs);

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

fn sanitize_for_claude(root: &Value, schema: &Value) -> Value {
    let mut seen_refs = std::collections::HashSet::new();
    sanitize_for_claude_inner(root, schema, &mut seen_refs)
}

fn fallback_schema() -> Value {
    let mut out = serde_json::Map::new();
    out.insert("type".to_string(), Value::String("object".to_string()));
    out.insert(
        "properties".to_string(),
        Value::Object(serde_json::Map::new()),
    );
    Value::Object(out)
}

fn sanitize_for_claude_inner(
    root: &Value,
    schema: &Value,
    seen_refs: &mut std::collections::HashSet<String>,
) -> Value {
    if let Some(reference) = schema.get("$ref").and_then(|v| v.as_str()) {
        if !seen_refs.insert(reference.to_string()) {
            return fallback_schema();
        }
        if let Some(resolved) = resolve_ref(root, reference) {
            let sanitized = sanitize_for_claude_inner(root, resolved, seen_refs);
            seen_refs.remove(reference);
            return sanitized;
        }
        seen_refs.remove(reference);
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
        return merge_union_schemas(root, union, seen_refs);
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
            | "deprecated" => {}
            "type" => {
                out.insert("type".to_string(), normalize_type(value));
            }
            "properties" => {
                if let Some(props) = value.as_object() {
                    let mut sanitized_props = serde_json::Map::new();
                    for (prop_key, prop_value) in props {
                        sanitized_props.insert(
                            prop_key.clone(),
                            sanitize_for_claude_inner(root, prop_value, seen_refs),
                        );
                    }
                    out.insert("properties".to_string(), Value::Object(sanitized_props));
                }
            }
            "items" => {
                if let Some(items) = value.as_array() {
                    let merged = merge_union_schemas(root, items, seen_refs);
                    out.insert("items".to_string(), merged);
                } else {
                    out.insert(
                        "items".to_string(),
                        sanitize_for_claude_inner(root, value, seen_refs),
                    );
                }
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
                out.insert(
                    key.clone(),
                    sanitize_for_claude_inner(root, value, seen_refs),
                );
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
        } else if out.contains_key("items") {
            out.insert("type".to_string(), Value::String("array".to_string()));
        } else if let Some(enum_values) = out.get("enum").and_then(|v| v.as_array())
            && let Some(inferred) = infer_type_from_enum(enum_values)
        {
            out.insert("type".to_string(), Value::String(inferred));
        }
    }

    Value::Object(out)
}

fn default_cache_type() -> String {
    "ephemeral".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sanitize_handles_recursive_ref() {
        let schema = json!({
            "$defs": {
                "node": {
                    "type": "object",
                    "properties": {
                        "next": { "$ref": "#/$defs/node" }
                    }
                }
            },
            "$ref": "#/$defs/node"
        });

        let sanitized = sanitize_for_claude(&schema, &schema);
        let next = sanitized
            .get("properties")
            .and_then(|v| v.get("next"))
            .and_then(|v| v.get("type"))
            .and_then(|v| v.as_str());

        assert_eq!(next, Some("object"));
    }

    #[test]
    fn sanitize_collapses_tuple_items() {
        let schema = json!({
            "type": "array",
            "items": [
                { "type": "string" },
                { "type": "number" }
            ]
        });

        let sanitized = sanitize_for_claude(&schema, &schema);
        let items = sanitized.get("items");

        assert!(matches!(items, Some(Value::Object(_))));
    }

    #[test]
    fn sanitize_removes_unsupported_keywords() {
        let schema = json!({
            "type": "object",
            "title": "ignored",
            "additionalProperties": false,
            "properties": {
                "name": {
                    "type": "string",
                    "pattern": "^[a-z]+$",
                    "default": "x"
                }
            }
        });

        let sanitized = sanitize_for_claude(&schema, &schema);
        let expected = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            }
        });

        assert_eq!(sanitized, expected);
    }

    #[test]
    fn sanitize_converts_const_to_enum_with_type() {
        let schema = json!({
            "const": "fixed"
        });

        let sanitized = sanitize_for_claude(&schema, &schema);
        let expected = json!({
            "enum": ["fixed"],
            "type": "string"
        });

        assert_eq!(sanitized, expected);
    }

    #[test]
    fn sanitize_filters_null_enum_values() {
        let schema = json!({
            "enum": ["a", null, "b"]
        });

        let sanitized = sanitize_for_claude(&schema, &schema);
        let expected = json!({
            "enum": ["a", "b"],
            "type": "string"
        });

        assert_eq!(sanitized, expected);
    }

    #[test]
    fn sanitize_decodes_json_pointer_refs() {
        let schema = json!({
            "$defs": {
                "a/b": { "type": "string" }
            },
            "$ref": "#/$defs/a~1b"
        });

        let sanitized = sanitize_for_claude(&schema, &schema);
        let expected = json!({
            "type": "string"
        });

        assert_eq!(sanitized, expected);
    }

    #[test]
    fn sanitize_merges_union_properties_and_required() {
        let schema = json!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "a": { "type": "string" },
                        "b": { "type": "string" }
                    },
                    "required": ["a"]
                },
                {
                    "type": "object",
                    "properties": {
                        "a": { "type": "string" },
                        "c": { "type": "string" }
                    },
                    "required": ["a", "c"]
                }
            ]
        });

        let sanitized = sanitize_for_claude(&schema, &schema);
        let expected = json!({
            "type": "object",
            "properties": {
                "a": { "type": "string" },
                "b": { "type": "string" },
                "c": { "type": "string" }
            },
            "required": ["a"]
        });

        assert_eq!(sanitized, expected);
    }

    #[test]
    fn sanitize_infers_array_type_from_items() {
        let schema = json!({
            "items": {
                "type": "string"
            }
        });

        let sanitized = sanitize_for_claude(&schema, &schema);
        let expected = json!({
            "type": "array",
            "items": { "type": "string" }
        });

        assert_eq!(sanitized, expected);
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CacheControl {
    #[serde(rename = "type", default = "default_cache_type")]
    cache_type: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum ClaudeContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: Vec<ClaudeContentBlock>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        signature: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking {
        data: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
struct ClaudeUsage {
    #[serde(rename = "input_tokens")]
    input: usize,
    #[serde(rename = "output_tokens")]
    output: usize,
    #[serde(rename = "cache_creation_input_tokens")]
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_creation_input: Option<usize>,
    #[serde(rename = "cache_read_input_tokens")]
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_read_input: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClaudeStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart {
        #[expect(dead_code)]
        message: ClaudeMessageStart,
    },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: ClaudeContentBlockStart,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: ClaudeDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta {
        #[expect(dead_code)]
        delta: ClaudeMessageDeltaData,
        #[expect(dead_code)]
        #[serde(default)]
        usage: Option<ClaudeUsage>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: ClaudeStreamError },
}

#[derive(Debug, Deserialize)]
struct ClaudeMessageStart {
    #[expect(dead_code)]
    #[serde(default)]
    id: String,
    #[expect(dead_code)]
    #[serde(default)]
    model: String,
}

#[derive(Debug, Deserialize)]
struct ClaudeContentBlockStart {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClaudeDelta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
    #[serde(rename = "signature_delta")]
    Signature { signature: String },
}

#[derive(Debug, Deserialize)]
struct ClaudeMessageDeltaData {
    #[expect(dead_code)]
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeStreamError {
    #[serde(default)]
    message: String,
    #[serde(rename = "type", default)]
    error_type: String,
}

impl AnthropicClient {
    pub fn new(api_key: &str) -> Result<Self, ApiError> {
        Self::with_api_key(api_key)
    }

    pub fn with_api_key(api_key: &str) -> Result<Self, ApiError> {
        Ok(Self {
            http_client: Self::build_http_client()?,
            auth: AuthMode::ApiKey(api_key.to_string()),
        })
    }

    pub fn with_directive(directive: AnthropicAuth) -> Result<Self, ApiError> {
        Ok(Self {
            http_client: Self::build_http_client()?,
            auth: AuthMode::Directive(directive),
        })
    }

    fn build_http_client() -> Result<reqwest::Client, ApiError> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "anthropic-version",
            header::HeaderValue::from_static("2023-06-01"),
        );
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );

        reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(ApiError::Network)
    }

    async fn auth_headers(
        &self,
        ctx: AuthHeaderContext,
    ) -> Result<Vec<(String, String)>, ApiError> {
        match &self.auth {
            AuthMode::ApiKey(key) => Ok(vec![("x-api-key".to_string(), key.clone())]),
            AuthMode::Directive(directive) => {
                let header_pairs = directive
                    .headers
                    .headers(ctx)
                    .await
                    .map_err(|e| ApiError::AuthError(e.to_string()))?;
                Ok(header_pairs
                    .into_iter()
                    .map(|pair| (pair.name, pair.value))
                    .collect())
            }
        }
    }

    async fn on_auth_error(
        &self,
        status: u16,
        body: &str,
        request_kind: RequestKind,
    ) -> Result<AuthErrorAction, ApiError> {
        let AuthMode::Directive(directive) = &self.auth else {
            return Ok(AuthErrorAction::NoAction);
        };
        let context = AuthErrorContext {
            status: Some(status),
            body_snippet: Some(truncate_body(body)),
            request_kind,
        };
        directive
            .headers
            .on_auth_error(context)
            .await
            .map_err(|e| ApiError::AuthError(e.to_string()))
    }

    fn request_url(&self) -> Result<String, ApiError> {
        let AuthMode::Directive(directive) = &self.auth else {
            return Ok(API_URL.to_string());
        };

        let Some(query_params) = &directive.query_params else {
            return Ok(API_URL.to_string());
        };

        if query_params.is_empty() {
            return Ok(API_URL.to_string());
        }

        let mut url = url::Url::parse(API_URL)
            .map_err(|e| ApiError::Configuration(format!("Invalid API_URL '{API_URL}': {e}")))?;
        for param in query_params {
            url.query_pairs_mut().append_pair(&param.name, &param.value);
        }
        Ok(url.to_string())
    }
}

// Conversion functions start
fn convert_messages(messages: Vec<AppMessage>) -> Result<Vec<ClaudeMessage>, ApiError> {
    let claude_messages: Result<Vec<ClaudeMessage>, ApiError> =
        messages.into_iter().map(convert_single_message).collect();

    // Filter out any User messages that have empty content after removing app commands
    claude_messages.map(|messages| {
        messages
            .into_iter()
            .filter(|msg| {
                match &msg.content {
                    ClaudeMessageContent::Text { content } => !content.trim().is_empty(),
                    ClaudeMessageContent::StructuredContent { .. } => true, // Keep all non-text messages
                }
            })
            .collect()
    })
}

fn convert_single_message(msg: AppMessage) -> Result<ClaudeMessage, ApiError> {
    match &msg.data {
        crate::app::conversation::MessageData::User { content, .. } => {
            // Convert UserContent to Claude format
            let combined_text = content
                .iter()
                .map(|user_content| match user_content {
                    UserContent::Text { text } => text.clone(),
                    UserContent::Image { image } => {
                        format!("[Image: {}]", image.mime_type)
                    }
                    UserContent::CommandExecution {
                        command,
                        stdout,
                        stderr,
                        exit_code,
                    } => UserContent::format_command_execution_as_xml(
                        command, stdout, stderr, *exit_code,
                    ),
                })
                .collect::<Vec<_>>()
                .join("\n");

            Ok(ClaudeMessage {
                role: ClaudeMessageRole::User,
                content: ClaudeMessageContent::Text {
                    content: combined_text,
                },
                id: Some(msg.id.clone()),
            })
        }
        crate::app::conversation::MessageData::Assistant { content, .. } => {
            // Convert AssistantContent to Claude blocks
            let claude_blocks: Vec<ClaudeContentBlock> = content
                .iter()
                .filter_map(|assistant_content| match assistant_content {
                    AssistantContent::Text { text } => {
                        if text.trim().is_empty() {
                            None
                        } else {
                            Some(ClaudeContentBlock::Text {
                                text: text.clone(),
                                cache_control: None,
                                extra: Default::default(),
                            })
                        }
                    }
                    AssistantContent::Image { image } => Some(ClaudeContentBlock::Text {
                        text: format!("[Image: {}]", image.mime_type),
                        cache_control: None,
                        extra: Default::default(),
                    }),
                    AssistantContent::ToolCall { tool_call, .. } => {
                        Some(ClaudeContentBlock::ToolUse {
                            id: tool_call.id.clone(),
                            name: tool_call.name.clone(),
                            input: tool_call.parameters.clone(),
                            cache_control: None,
                            extra: Default::default(),
                        })
                    }
                    AssistantContent::Thought { thought } => {
                        match thought {
                            ThoughtContent::Signed { text, signature } => {
                                Some(ClaudeContentBlock::Thinking {
                                    thinking: text.clone(),
                                    signature: signature.clone(),
                                    cache_control: None,
                                    extra: Default::default(),
                                })
                            }
                            ThoughtContent::Redacted { data } => {
                                Some(ClaudeContentBlock::RedactedThinking {
                                    data: data.clone(),
                                    cache_control: None,
                                    extra: Default::default(),
                                })
                            }
                            ThoughtContent::Simple { text } => {
                                // Claude doesn't have a simple thought type, convert to text
                                Some(ClaudeContentBlock::Text {
                                    text: format!("[Thought: {text}]"),
                                    cache_control: None,
                                    extra: Default::default(),
                                })
                            }
                        }
                    }
                })
                .collect();

            if claude_blocks.is_empty() {
                debug!("No content blocks found: {:?}", content);
                Err(ApiError::InvalidRequest {
                    provider: "anthropic".to_string(),
                    details: format!(
                        "Assistant message ID {} resulted in no valid content blocks",
                        msg.id
                    ),
                })
            } else {
                let claude_blocks = ensure_thinking_first(claude_blocks);
                let claude_content = if claude_blocks.len() == 1 {
                    if let Some(ClaudeContentBlock::Text { text, .. }) = claude_blocks.first() {
                        ClaudeMessageContent::Text {
                            content: text.clone(),
                        }
                    } else {
                        ClaudeMessageContent::StructuredContent {
                            content: ClaudeStructuredContent(claude_blocks),
                        }
                    }
                } else {
                    ClaudeMessageContent::StructuredContent {
                        content: ClaudeStructuredContent(claude_blocks),
                    }
                };

                Ok(ClaudeMessage {
                    role: ClaudeMessageRole::Assistant,
                    content: claude_content,
                    id: Some(msg.id.clone()),
                })
            }
        }
        crate::app::conversation::MessageData::Tool {
            tool_use_id,
            result,
            ..
        } => {
            // Convert ToolResult to Claude format
            // Claude expects tool results as User messages
            let (result_text, is_error) = if let ToolResult::Error(e) = result {
                (e.to_string(), Some(true))
            } else {
                // For all other variants, use llm_format
                let text = result.llm_format();
                let text = if text.trim().is_empty() {
                    "(No output)".to_string()
                } else {
                    text
                };
                (text, None)
            };

            let claude_blocks = vec![ClaudeContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: vec![ClaudeContentBlock::Text {
                    text: result_text,
                    cache_control: None,
                    extra: Default::default(),
                }],
                is_error,
                cache_control: None,
                extra: Default::default(),
            }];

            Ok(ClaudeMessage {
                role: ClaudeMessageRole::User, // Tool results are sent as User messages in Claude
                content: ClaudeMessageContent::StructuredContent {
                    content: ClaudeStructuredContent(claude_blocks),
                },
                id: Some(msg.id.clone()),
            })
        }
    }
}
// Conversion functions end

fn ensure_thinking_first(blocks: Vec<ClaudeContentBlock>) -> Vec<ClaudeContentBlock> {
    let mut thinking_blocks = Vec::new();
    let mut other_blocks = Vec::new();

    for block in blocks {
        match block {
            ClaudeContentBlock::Thinking { .. } | ClaudeContentBlock::RedactedThinking { .. } => {
                thinking_blocks.push(block);
            }
            _ => other_blocks.push(block),
        }
    }

    if thinking_blocks.is_empty() {
        other_blocks
    } else {
        thinking_blocks.extend(other_blocks);
        thinking_blocks
    }
}

// Convert Claude's content blocks to our provider-agnostic format
fn convert_claude_content(claude_blocks: Vec<ClaudeContentBlock>) -> Vec<AssistantContent> {
    claude_blocks
        .into_iter()
        .filter_map(|block| match block {
            ClaudeContentBlock::Text { text, .. } => Some(AssistantContent::Text { text }),
            ClaudeContentBlock::ToolUse {
                id, name, input, ..
            } => Some(AssistantContent::ToolCall {
                tool_call: steer_tools::ToolCall {
                    id,
                    name,
                    parameters: input,
                },
                thought_signature: None,
            }),
            ClaudeContentBlock::ToolResult { .. } => {
                warn!("Unexpected ToolResult block received in Claude response content");
                None
            }
            ClaudeContentBlock::Thinking {
                thinking,
                signature,
                ..
            } => Some(AssistantContent::Thought {
                thought: ThoughtContent::Signed {
                    text: thinking,
                    signature,
                },
            }),
            ClaudeContentBlock::RedactedThinking { data, .. } => Some(AssistantContent::Thought {
                thought: ThoughtContent::Redacted { data },
            }),
            ClaudeContentBlock::Unknown => {
                warn!("Unknown content block received in Claude response content");
                None
            }
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
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        let mut claude_messages = convert_messages(messages)?;
        let tools = tools.map(|tools| tools.into_iter().map(ClaudeTool::from).collect());

        if claude_messages.is_empty() {
            return Err(ApiError::InvalidRequest {
                provider: self.name().to_string(),
                details: "No messages provided".to_string(),
            });
        }

        let last_message = claude_messages
            .last_mut()
            .ok_or_else(|| ApiError::InvalidRequest {
                provider: self.name().to_string(),
                details: "No messages provided".to_string(),
            })?;
        let cache_setting = Some(CacheControl {
            cache_type: "ephemeral".to_string(),
        });

        let instruction_policy = match &self.auth {
            AuthMode::Directive(directive) => directive.instruction_policy.as_ref(),
            AuthMode::ApiKey(_) => None,
        };
        let system_text = apply_instruction_policy(system, instruction_policy);
        let system_content = build_system_content(system_text, cache_setting.clone());

        match &mut last_message.content {
            ClaudeMessageContent::StructuredContent { content } => {
                for block in &mut content.0 {
                    if let ClaudeContentBlock::ToolResult { cache_control, .. } = block {
                        cache_control.clone_from(&cache_setting);
                    }
                }
            }
            ClaudeMessageContent::Text { content } => {
                let text_content = content.clone();
                last_message.content = ClaudeMessageContent::StructuredContent {
                    content: ClaudeStructuredContent(vec![ClaudeContentBlock::Text {
                        text: text_content,
                        cache_control: cache_setting,
                        extra: Default::default(),
                    }]),
                };
            }
        }

        // Extract model-specific logic using ModelId
        let supports_thinking = call_options
            .as_ref()
            .and_then(|opts| opts.thinking_config.as_ref())
            .is_some_and(|tc| tc.enabled);

        let request = if supports_thinking {
            // Use catalog/call options to configure thinking budget when provided
            let budget = call_options
                .as_ref()
                .and_then(|o| o.thinking_config)
                .and_then(|tc| tc.budget_tokens)
                .unwrap_or(4000);
            let thinking = Some(Thinking {
                thinking_type: ThinkingType::Enabled,
                budget_tokens: budget,
            });
            CompletionRequest {
                model: model_id.id.clone(), // Use the model ID string
                messages: claude_messages,
                max_tokens: call_options
                    .as_ref()
                    .and_then(|o| o.max_tokens)
                    .map_or(32_000, |v| v as usize),
                system: system_content.clone(),
                tools,
                temperature: call_options
                    .as_ref()
                    .and_then(|o| o.temperature)
                    .or(Some(1.0)),
                top_p: call_options.as_ref().and_then(|o| o.top_p),
                top_k: None,
                stream: None,
                thinking,
            }
        } else {
            CompletionRequest {
                model: model_id.id.clone(), // Use the model ID string
                messages: claude_messages,
                max_tokens: call_options
                    .as_ref()
                    .and_then(|o| o.max_tokens)
                    .map_or(8000, |v| v as usize),
                system: system_content,
                tools,
                temperature: call_options
                    .as_ref()
                    .and_then(|o| o.temperature)
                    .or(Some(0.7)),
                top_p: call_options.as_ref().and_then(|o| o.top_p),
                top_k: None,
                stream: None,
                thinking: None,
            }
        };

        let auth_ctx = auth_header_context(model_id, RequestKind::Complete);
        let mut attempts = 0;

        loop {
            let auth_headers = self.auth_headers(auth_ctx.clone()).await?;
            let url = self.request_url()?;
            let mut request_builder = self.http_client.post(&url).json(&request);

            for (name, value) in auth_headers {
                request_builder = request_builder.header(&name, &value);
            }

            if supports_thinking && matches!(&self.auth, AuthMode::ApiKey(_)) {
                request_builder =
                    request_builder.header("anthropic-beta", "interleaved-thinking-2025-05-14");
            }

            let response = tokio::select! {
                biased;
                () = token.cancelled() => {
                    debug!(target: "claude::complete", "Cancellation token triggered before sending request.");
                    return Err(ApiError::Cancelled{ provider: self.name().to_string()});
                }
                res = request_builder.send() => {
                    res?
                }
            };

            if token.is_cancelled() {
                debug!(target: "claude::complete", "Cancellation token triggered after sending request, before status check.");
                return Err(ApiError::Cancelled {
                    provider: self.name().to_string(),
                });
            }

            let status = response.status();
            if !status.is_success() {
                let error_text = tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        debug!(target: "claude::complete", "Cancellation token triggered while reading error response body.");
                        return Err(ApiError::Cancelled{ provider: self.name().to_string()});
                    }
                    text_res = response.text() => {
                        text_res?
                    }
                };

                if is_auth_status(status) && matches!(&self.auth, AuthMode::Directive(_)) {
                    let action = self
                        .on_auth_error(status.as_u16(), &error_text, RequestKind::Complete)
                        .await?;
                    if matches!(action, AuthErrorAction::RetryOnce) && attempts == 0 {
                        attempts += 1;
                        continue;
                    }
                    return Err(ApiError::AuthenticationFailed {
                        provider: self.name().to_string(),
                        details: error_text,
                    });
                }

                return Err(match status.as_u16() {
                    401 | 403 => ApiError::AuthenticationFailed {
                        provider: self.name().to_string(),
                        details: error_text,
                    },
                    429 => ApiError::RateLimited {
                        provider: self.name().to_string(),
                        details: error_text,
                    },
                    400..=499 => ApiError::InvalidRequest {
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

            let response_text = tokio::select! {
                biased;
                () = token.cancelled() => {
                    debug!(target: "claude::complete", "Cancellation token triggered while reading successful response body.");
                    return Err(ApiError::Cancelled { provider: self.name().to_string() });
                }
                text_res = response.text() => {
                    text_res?
                }
            };

            let claude_completion: ClaudeCompletionResponse = serde_json::from_str(&response_text)
                .map_err(|e| ApiError::ResponseParsingError {
                    provider: self.name().to_string(),
                    details: format!("Error: {e}, Body: {response_text}"),
                })?;
            let completion = CompletionResponse {
                content: convert_claude_content(claude_completion.content),
            };

            return Ok(completion);
        }
    }

    async fn stream_complete(
        &self,
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionStream, ApiError> {
        let mut claude_messages = convert_messages(messages)?;
        let tools = tools.map(|tools| tools.into_iter().map(ClaudeTool::from).collect());

        if claude_messages.is_empty() {
            return Err(ApiError::InvalidRequest {
                provider: self.name().to_string(),
                details: "No messages provided".to_string(),
            });
        }

        let last_message = claude_messages
            .last_mut()
            .ok_or_else(|| ApiError::InvalidRequest {
                provider: self.name().to_string(),
                details: "No messages provided".to_string(),
            })?;
        let cache_setting = Some(CacheControl {
            cache_type: "ephemeral".to_string(),
        });

        let instruction_policy = match &self.auth {
            AuthMode::Directive(directive) => directive.instruction_policy.as_ref(),
            AuthMode::ApiKey(_) => None,
        };
        let system_text = apply_instruction_policy(system, instruction_policy);
        let system_content = build_system_content(system_text, cache_setting.clone());

        match &mut last_message.content {
            ClaudeMessageContent::StructuredContent { content } => {
                for block in &mut content.0 {
                    if let ClaudeContentBlock::ToolResult { cache_control, .. } = block {
                        cache_control.clone_from(&cache_setting);
                    }
                }
            }
            ClaudeMessageContent::Text { content } => {
                let text_content = content.clone();
                last_message.content = ClaudeMessageContent::StructuredContent {
                    content: ClaudeStructuredContent(vec![ClaudeContentBlock::Text {
                        text: text_content,
                        cache_control: cache_setting,
                        extra: Default::default(),
                    }]),
                };
            }
        }

        let supports_thinking = call_options
            .as_ref()
            .and_then(|opts| opts.thinking_config.as_ref())
            .is_some_and(|tc| tc.enabled);

        let request = if supports_thinking {
            let budget = call_options
                .as_ref()
                .and_then(|o| o.thinking_config)
                .and_then(|tc| tc.budget_tokens)
                .unwrap_or(4000);
            let thinking = Some(Thinking {
                thinking_type: ThinkingType::Enabled,
                budget_tokens: budget,
            });
            CompletionRequest {
                model: model_id.id.clone(),
                messages: claude_messages,
                max_tokens: call_options
                    .as_ref()
                    .and_then(|o| o.max_tokens)
                    .map_or(32_000, |v| v as usize),
                system: system_content.clone(),
                tools,
                temperature: call_options
                    .as_ref()
                    .and_then(|o| o.temperature)
                    .or(Some(1.0)),
                top_p: call_options.as_ref().and_then(|o| o.top_p),
                top_k: None,
                stream: Some(true),
                thinking,
            }
        } else {
            CompletionRequest {
                model: model_id.id.clone(),
                messages: claude_messages,
                max_tokens: call_options
                    .as_ref()
                    .and_then(|o| o.max_tokens)
                    .map_or(8000, |v| v as usize),
                system: system_content,
                tools,
                temperature: call_options
                    .as_ref()
                    .and_then(|o| o.temperature)
                    .or(Some(0.7)),
                top_p: call_options.as_ref().and_then(|o| o.top_p),
                top_k: None,
                stream: Some(true),
                thinking: None,
            }
        };

        let auth_ctx = auth_header_context(model_id, RequestKind::Stream);
        let mut attempts = 0;

        loop {
            let auth_headers = self.auth_headers(auth_ctx.clone()).await?;
            let url = self.request_url()?;
            let mut request_builder = self.http_client.post(&url).json(&request);

            for (name, value) in auth_headers {
                request_builder = request_builder.header(&name, &value);
            }

            if supports_thinking && matches!(&self.auth, AuthMode::ApiKey(_)) {
                request_builder =
                    request_builder.header("anthropic-beta", "interleaved-thinking-2025-05-14");
            }

            let response = tokio::select! {
                biased;
                () = token.cancelled() => {
                    return Err(ApiError::Cancelled { provider: self.name().to_string() });
                }
                res = request_builder.send() => {
                    res?
                }
            };

            let status = response.status();
            if !status.is_success() {
                let error_text = tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        return Err(ApiError::Cancelled { provider: self.name().to_string() });
                    }
                    text_res = response.text() => {
                        text_res?
                    }
                };

                if is_auth_status(status) && matches!(&self.auth, AuthMode::Directive(_)) {
                    let action = self
                        .on_auth_error(status.as_u16(), &error_text, RequestKind::Stream)
                        .await?;
                    if matches!(action, AuthErrorAction::RetryOnce) && attempts == 0 {
                        attempts += 1;
                        continue;
                    }
                    return Err(ApiError::AuthenticationFailed {
                        provider: self.name().to_string(),
                        details: error_text,
                    });
                }

                return Err(match status.as_u16() {
                    401 | 403 => ApiError::AuthenticationFailed {
                        provider: self.name().to_string(),
                        details: error_text,
                    },
                    429 => ApiError::RateLimited {
                        provider: self.name().to_string(),
                        details: error_text,
                    },
                    400..=499 => ApiError::InvalidRequest {
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

            let stream = convert_claude_stream(sse_stream, token);

            return Ok(Box::pin(stream));
        }
    }
}

fn auth_header_context(model_id: &ModelId, request_kind: RequestKind) -> AuthHeaderContext {
    AuthHeaderContext {
        model_id: Some(AuthModelId {
            provider_id: AuthProviderId(model_id.provider.as_str().to_string()),
            model_id: model_id.id.clone(),
        }),
        request_kind,
    }
}

fn is_auth_status(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    )
}

fn truncate_body(body: &str) -> String {
    const LIMIT: usize = 512;
    let mut chars = body.chars();
    let truncated: String = chars.by_ref().take(LIMIT).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn apply_instruction_policy(
    system: Option<SystemContext>,
    policy: Option<&InstructionPolicy>,
) -> Option<String> {
    let base = system.as_ref().and_then(|context| {
        let trimmed = context.prompt.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    let context = system
        .as_ref()
        .and_then(|context| context.render_with_prompt(base.clone()));

    match policy {
        None => context,
        Some(InstructionPolicy::Prefix(prefix)) => {
            if let Some(context) = context {
                Some(format!("{prefix}\n{context}"))
            } else {
                Some(prefix.clone())
            }
        }
        Some(InstructionPolicy::DefaultIfEmpty(default)) => {
            if context.is_some() {
                context
            } else {
                Some(default.clone())
            }
        }
        Some(InstructionPolicy::Override(override_text)) => {
            let mut combined = override_text.clone();
            if let Some(system) = system.as_ref() {
                let overlay = system.prompt.trim();
                if !overlay.is_empty() {
                    combined.push_str("\n\n## Operating Mode\n");
                    combined.push_str(overlay);
                }

                let env = system
                    .environment
                    .as_ref()
                    .map(|env| env.as_context())
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty());
                if let Some(env) = env {
                    combined.push_str("\n\n");
                    combined.push_str(&env);
                }
            }
            Some(combined)
        }
    }
}

fn build_system_content(
    system: Option<String>,
    cache_setting: Option<CacheControl>,
) -> Option<System> {
    system.map(|text| {
        System::Content(vec![SystemContentBlock {
            content_type: "text".to_string(),
            text,
            cache_control: cache_setting,
        }])
    })
}

#[derive(Debug)]
enum BlockState {
    Text {
        text: String,
    },
    Thinking {
        text: String,
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    Unknown,
}

fn block_state_to_content(state: BlockState) -> Option<AssistantContent> {
    match state {
        BlockState::Text { text } => {
            if text.is_empty() {
                None
            } else {
                Some(AssistantContent::Text { text })
            }
        }
        BlockState::Thinking { text, signature } => {
            if text.is_empty() {
                None
            } else {
                let thought = if let Some(sig) = signature {
                    ThoughtContent::Signed {
                        text,
                        signature: sig,
                    }
                } else {
                    ThoughtContent::Simple { text }
                };
                Some(AssistantContent::Thought { thought })
            }
        }
        BlockState::ToolUse { id, name, input } => {
            if id.is_empty() || name.is_empty() {
                None
            } else {
                let parameters: serde_json::Value = serde_json::from_str(&input)
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                Some(AssistantContent::ToolCall {
                    tool_call: ToolCall {
                        id,
                        name,
                        parameters,
                    },
                    thought_signature: None,
                })
            }
        }
        BlockState::Unknown => None,
    }
}

fn convert_claude_stream(
    sse_stream: crate::api::sse::SseStream,
    token: CancellationToken,
) -> impl futures_core::Stream<Item = StreamChunk> + Send {
    async_stream::stream! {
        let mut block_states: std::collections::HashMap<usize, BlockState> =
            std::collections::HashMap::new();
        let mut completed_content: Vec<AssistantContent> = Vec::new();

        tokio::pin!(sse_stream);

        while let Some(event_result) = sse_stream.next().await {
            if token.is_cancelled() {
                yield StreamChunk::Error(StreamError::Cancelled);
                break;
            }

            let event = match event_result {
                Ok(e) => e,
                Err(e) => {
                    yield StreamChunk::Error(StreamError::SseParse(e));
                    break;
                }
            };

            let parsed: Result<ClaudeStreamEvent, _> = serde_json::from_str(&event.data);
            let stream_event = match parsed {
                Ok(e) => e,
                Err(_) => continue,
            };

            match stream_event {
                ClaudeStreamEvent::ContentBlockStart { index, content_block } => {
                    match content_block.block_type.as_str() {
                        "text" => {
                            let text = content_block.text.unwrap_or_default();
                            if !text.is_empty() {
                                yield StreamChunk::TextDelta(text.clone());
                            }
                            block_states.insert(index, BlockState::Text { text });
                        }
                        "thinking" => {
                            block_states.insert(
                                index,
                                BlockState::Thinking {
                                    text: String::new(),
                                    signature: None,
                                },
                            );
                        }
                        "tool_use" => {
                            let id = content_block.id.unwrap_or_default();
                            let name = content_block.name.unwrap_or_default();
                            if !id.is_empty() && !name.is_empty() {
                                yield StreamChunk::ToolUseStart {
                                    id: id.clone(),
                                    name: name.clone(),
                                };
                            }
                            block_states.insert(
                                index,
                                BlockState::ToolUse {
                                    id,
                                    name,
                                    input: String::new(),
                                },
                            );
                        }
                        _ => {
                            block_states.insert(index, BlockState::Unknown);
                        }
                    }
                }
                ClaudeStreamEvent::ContentBlockDelta { index, delta } => match delta {
                    ClaudeDelta::Text { text } => {
                        match block_states.get_mut(&index) {
                            Some(BlockState::Text { text: buf }) => buf.push_str(&text),
                            _ => {
                                block_states.insert(index, BlockState::Text { text: text.clone() });
                            }
                        }
                        yield StreamChunk::TextDelta(text);
                    }
                    ClaudeDelta::Thinking { thinking } => {
                        match block_states.get_mut(&index) {
                            Some(BlockState::Thinking { text, .. }) => text.push_str(&thinking),
                            _ => {
                                block_states.insert(
                                    index,
                                    BlockState::Thinking {
                                        text: thinking.clone(),
                                        signature: None,
                                    },
                                );
                            }
                        }
                        yield StreamChunk::ThinkingDelta(thinking);
                    }
                    ClaudeDelta::Signature { signature } => {
                        if let Some(BlockState::Thinking { signature: sig, .. }) =
                            block_states.get_mut(&index)
                        {
                            *sig = Some(signature);
                        }
                    }
                    ClaudeDelta::InputJson { partial_json } => {
                        if let Some(BlockState::ToolUse { id, input, .. }) =
                            block_states.get_mut(&index)
                        {
                            input.push_str(&partial_json);
                            if !id.is_empty() {
                                yield StreamChunk::ToolUseInputDelta {
                                    id: id.clone(),
                                    delta: partial_json,
                                };
                            }
                        }
                    }
                },
                ClaudeStreamEvent::ContentBlockStop { index } => {
                    if let Some(state) = block_states.remove(&index)
                        && let Some(content) = block_state_to_content(state)
                    {
                        completed_content.push(content);
                    }
                    yield StreamChunk::ContentBlockStop { index };
                }
                ClaudeStreamEvent::MessageStop => {
                    if !block_states.is_empty() {
                        tracing::warn!(
                            target: "anthropic::stream",
                            "MessageStop received with {} unfinished content blocks",
                            block_states.len()
                        );
                    }
                    let content = std::mem::take(&mut completed_content);
                    yield StreamChunk::MessageComplete(CompletionResponse { content });
                    break;
                }
                ClaudeStreamEvent::Error { error } => {
                    yield StreamChunk::Error(StreamError::Provider {
                        provider: "anthropic".into(),
                        error_type: error.error_type,
                        message: error.message,
                    });
                }
                ClaudeStreamEvent::MessageStart { .. }
                | ClaudeStreamEvent::MessageDelta { .. }
                | ClaudeStreamEvent::Ping => {}
            }
        }
    }
}
