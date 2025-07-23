use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSchema {
    pub properties: serde_json::Map<String, Value>,
    pub required: Vec<String>,
    #[serde(rename = "type")]
    pub schema_type: String,
}

impl From<schemars::Schema> for InputSchema {
    fn from(schema: schemars::Schema) -> Self {
        let schema_obj = schema.as_object().unwrap();
        Self {
            properties: schema_obj
                .get("properties")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default(),
            required: schema_obj
                .get("required")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            schema_type: schema_obj
                .get("type")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema: InputSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub parameters: Value,
    pub id: String,
}
