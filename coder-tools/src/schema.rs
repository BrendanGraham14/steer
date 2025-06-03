use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSchema {
    pub properties: serde_json::Map<String, Value>,
    pub required: Vec<String>,
    #[serde(rename = "type")]
    pub schema_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub parameters: Value,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub output: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(tool_call_id: String, output: String) -> Self {
        Self {
            tool_call_id,
            output,
            is_error: false,
        }
    }

    pub fn error(tool_call_id: String, error_message: String) -> Self {
        Self {
            tool_call_id,
            output: error_message,
            is_error: true,
        }
    }
}