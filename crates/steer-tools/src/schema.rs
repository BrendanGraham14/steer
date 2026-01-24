
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::error::Error as StdError;
use schemars::JsonSchema;
use crate::error::ToolExecutionError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InputSchema(Value);

impl InputSchema {
    pub fn new(schema: Value) -> Self {
        Self(schema)
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }

    pub fn summary(&self) -> InputSchemaSummary {
        InputSchemaSummary::from_value(&self.0)
    }

    pub fn object(properties: serde_json::Map<String, Value>, required: Vec<String>) -> Self {
        let mut schema = serde_json::Map::new();
        schema.insert("type".to_string(), Value::String("object".to_string()));
        schema.insert("properties".to_string(), Value::Object(properties));
        if !required.is_empty() {
            let required_values = required
                .into_iter()
                .map(Value::String)
                .collect::<Vec<_>>();
            schema.insert("required".to_string(), Value::Array(required_values));
        }
        Self(Value::Object(schema))
    }

    pub fn empty_object() -> Self {
        Self::object(Default::default(), Vec::new())
    }
}

impl From<Value> for InputSchema {
    fn from(schema: Value) -> Self {
        Self(schema)
    }
}

impl From<schemars::Schema> for InputSchema {
    fn from(schema: schemars::Schema) -> Self {
        let schema_value =
            serde_json::to_value(&schema).unwrap_or_else(|_| serde_json::Value::Null);
        Self(ensure_object_properties(schema_value))
    }
}

fn ensure_object_properties(schema: Value) -> Value {
    let mut schema = schema;
    if let Value::Object(obj) = &mut schema {
        let is_object = obj
            .get("type")
            .and_then(|v| v.as_str())
            .is_some_and(|t| t == "object");
        if is_object && !obj.contains_key("properties") {
            obj.insert("properties".to_string(), Value::Object(serde_json::Map::new()));
        }
    }
    schema
}

#[derive(Debug, Clone)]
pub struct InputSchemaSummary {
    pub properties: serde_json::Map<String, Value>,
    pub required: Vec<String>,
    pub schema_type: String,
}

impl InputSchemaSummary {
    fn from_value(schema: &Value) -> Self {
        let mut properties = serde_json::Map::new();
        let mut required = std::collections::BTreeSet::new();

        Self::merge_schema(schema, &mut properties, &mut required);

        let mut schema_type = schema
            .as_object()
            .and_then(|obj| obj.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        if schema_type.is_empty() && (!properties.is_empty() || !required.is_empty()) {
            schema_type = "object".to_string();
        }

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
                let summary = InputSchemaSummary::from_value(sub);
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
            let summary = InputSchemaSummary::from_value(sub);
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
    fn dispatch_agent_schema_includes_target() {
        let schema = schema_for!(crate::tools::dispatch_agent::DispatchAgentParams);
        let input_schema: InputSchema = schema.into();
        let summary = input_schema.summary();

        assert!(summary.properties.contains_key("prompt"));
        assert!(summary.properties.contains_key("target"));
        assert!(summary.required.contains(&"prompt".to_string()));
        assert!(summary.required.contains(&"target".to_string()));
        assert_eq!(summary.schema_type, "object");

        let root = input_schema.as_value();
        let target_schema = root
            .get("properties")
            .and_then(|v| v.get("target"))
            .expect("target schema should exist");
        let target_schema = resolve_ref(root, target_schema);
        let variants = target_schema
            .get("oneOf")
            .or_else(|| target_schema.get("anyOf"))
            .or_else(|| target_schema.get("allOf"))
            .and_then(|v| v.as_array())
            .expect("target should be a tagged union");

        let mut mode_values = Vec::new();
        for variant in variants {
            let mode_prop = variant
                .get("properties")
                .and_then(|v| v.get("mode"))
                .unwrap_or(&serde_json::Value::Null);
            mode_values.extend(super::extract_enum_values(mode_prop));
        }

        assert!(mode_values.contains(&serde_json::Value::String("new".to_string())));
        assert!(mode_values.contains(&serde_json::Value::String("resume".to_string())));
    }

    fn resolve_ref<'a>(root: &'a serde_json::Value, schema: &'a serde_json::Value) -> &'a serde_json::Value {
        let Some(reference) = schema.get("$ref").and_then(|v| v.as_str()) else {
            return schema;
        };
        let path = reference.strip_prefix("#/").unwrap_or(reference);
        let mut current = root;
        for segment in path.split('/') {
            let Some(next) = current.get(segment) else {
                return schema;
            };
            current = next;
        }
        current
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
