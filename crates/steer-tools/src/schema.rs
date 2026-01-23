
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::error::Error as StdError;
use schemars::JsonSchema;
use crate::error::ToolExecutionError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSchema {
    pub properties: serde_json::Map<String, Value>,
    pub required: Vec<String>,
    #[serde(rename = "type")]
    pub schema_type: String,
}

impl From<schemars::Schema> for InputSchema {
    fn from(schema: schemars::Schema) -> Self {
        let schema_value =
            serde_json::to_value(&schema).unwrap_or_else(|_| serde_json::Value::Null);
        let summary = SchemaSummary::from_value(&schema_value);
        Self {
            properties: summary.properties,
            required: summary.required,
            schema_type: summary.schema_type,
        }
    }
}

struct SchemaSummary {
    properties: serde_json::Map<String, Value>,
    required: Vec<String>,
    schema_type: String,
}

impl SchemaSummary {
    fn from_value(schema: &Value) -> Self {
        let mut properties = serde_json::Map::new();
        let mut required = std::collections::BTreeSet::new();
        let schema_type = schema
            .as_object()
            .and_then(|obj| obj.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        Self::merge_schema(schema, &mut properties, &mut required);

        Self {
            properties,
            required: required.into_iter().collect(),
            schema_type,
        }
    }

    fn merge_schema(
        schema: &Value,
        properties: &mut serde_json::Map<String, Value>,
        required: &mut std::collections::BTreeSet<String>,
    ) {
        let Some(obj) = schema.as_object() else {
            return;
        };

        if let Some(prop_obj) = obj.get("properties").and_then(|v| v.as_object()) {
            for (key, value) in prop_obj {
                merge_property(properties, key, value);
            }
        }

        if let Some(req) = obj.get("required").and_then(|v| v.as_array()) {
            for item in req {
                if let Some(name) = item.as_str() {
                    required.insert(name.to_string());
                }
            }
        }

        if let Some(all_of) = obj.get("allOf").and_then(|v| v.as_array()) {
            for sub in all_of {
                let summary = SchemaSummary::from_value(sub);
                for (key, value) in summary.properties {
                    merge_property(properties, &key, &value);
                }
                required.extend(summary.required);
            }
        }

        if let Some(one_of) = obj.get("oneOf").and_then(|v| v.as_array()) {
            Self::merge_one_of(one_of, properties, required);
        }

        if let Some(any_of) = obj.get("anyOf").and_then(|v| v.as_array()) {
            Self::merge_one_of(any_of, properties, required);
        }
    }

    fn merge_one_of(
        subschemas: &[Value],
        properties: &mut serde_json::Map<String, Value>,
        required: &mut std::collections::BTreeSet<String>,
    ) {
        let mut intersection: Option<std::collections::BTreeSet<String>> = None;

        for sub in subschemas {
            let summary = SchemaSummary::from_value(sub);
            for (key, value) in summary.properties {
                merge_property(properties, &key, &value);
            }

            let required_set: std::collections::BTreeSet<String> =
                summary.required.into_iter().collect();

            intersection = match intersection.take() {
                None => Some(required_set),
                Some(existing) => Some(
                    existing
                        .intersection(&required_set)
                        .cloned()
                        .collect::<std::collections::BTreeSet<String>>(),
                ),
            };
        }

        if let Some(required_set) = intersection {
            required.extend(required_set);
        }
    }
}

fn merge_property(
    properties: &mut serde_json::Map<String, Value>,
    key: &str,
    value: &Value,
) {
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
                obj.insert("enum".to_string(), Value::Array(combined));
            }
        }
    }
}

fn extract_enum_values(value: &Value) -> Vec<Value> {
    let Some(obj) = value.as_object() else {
        return Vec::new();
    };

    if let Some(enum_values) = obj.get("enum").and_then(|v| v.as_array()) {
        return enum_values.clone();
    }

    if let Some(const_value) = obj.get("const") {
        return vec![const_value.clone()];
    }

    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::InputSchema;
    use schemars::schema_for;

    #[test]
    fn dispatch_agent_schema_includes_mode() {
        let schema = schema_for!(crate::tools::dispatch_agent::DispatchAgentParams);
        let input_schema: InputSchema = schema.into();

        assert!(input_schema.properties.contains_key("prompt"));
        assert!(input_schema.properties.contains_key("mode"));
        assert!(input_schema.properties.contains_key("workspace"));
        assert!(input_schema.properties.contains_key("session_id"));
        assert!(input_schema.required.contains(&"prompt".to_string()));
        assert!(input_schema.required.contains(&"mode".to_string()));

        let mode_schema = input_schema
            .properties
            .get("mode")
            .and_then(|v| v.get("enum"))
            .and_then(|v| v.as_array())
            .expect("mode enum should exist");

        assert!(mode_schema.contains(&serde_json::Value::String("new".to_string())));
        assert!(mode_schema.contains(&serde_json::Value::String("resume".to_string())));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    pub description: String,
    pub input_schema: InputSchema,
}

pub trait ToolSpec {
    type Params: DeserializeOwned + JsonSchema + Send;
    type Result: Into<crate::result::ToolResult> + Send;
    type Error: StdError + Send + Sync + 'static;

    const NAME: &'static str;
    const DISPLAY_NAME: &'static str;

    fn execution_error(error: Self::Error) -> ToolExecutionError;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub parameters: Value,
    pub id: String,
}
