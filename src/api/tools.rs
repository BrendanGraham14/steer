use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Schema for tool inputs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSchema {
    pub properties: serde_json::Map<String, Value>,
    pub required: Vec<String>,
    #[serde(rename = "type")]
    pub schema_type: String,
}

/// A tool definition that Claude can use
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: InputSchema,
}

/// A tool call from Claude
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// A result from a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub output: String,
}

impl Tool {
    pub fn bash() -> Self {
        let mut properties = serde_json::Map::new();

        // Command property
        let mut command_prop = serde_json::Map::new();
        command_prop.insert(
            "description".to_string(),
            serde_json::json!("The command to execute"),
        );
        properties.insert(
            "command".to_string(),
            serde_json::Value::Object(command_prop),
        );

        // Timeout property
        let mut timeout_prop = serde_json::Map::new();
        timeout_prop.insert(
            "description".to_string(),
            serde_json::json!("Optional timeout in milliseconds (max 600000)"),
        );
        properties.insert(
            "timeout".to_string(),
            serde_json::Value::Object(timeout_prop),
        );

        Self {
            name: "Bash".to_string(),
            description: "Run a bash command in the terminal".to_string(),
            // tool_type: "custom".to_string(),
            input_schema: InputSchema {
                properties,
                schema_type: "object".to_string(),
                required: vec!["command".to_string()],
            },
        }
    }

    pub fn glob_tool() -> Self {
        let mut properties = serde_json::Map::new();

        // Pattern property
        let mut pattern_prop = serde_json::Map::new();
        pattern_prop.insert(
            "description".to_string(),
            serde_json::json!("The glob pattern to match files against"),
        );
        properties.insert(
            "pattern".to_string(),
            serde_json::Value::Object(pattern_prop),
        );

        // Path property
        let mut path_prop = serde_json::Map::new();
        path_prop.insert(
            "description".to_string(),
            serde_json::json!(
                "The directory to search in. Defaults to the current working directory."
            ),
        );
        properties.insert("path".to_string(), serde_json::Value::Object(path_prop));

        Self {
            name: "GlobTool".to_string(),
            description: "Find files by glob pattern".to_string(),
            // tool_type: "custom".to_string(),
            input_schema: InputSchema {
                properties,
                schema_type: "object".to_string(),
                required: vec!["pattern".to_string()],
            },
        }
    }

    pub fn grep_tool() -> Self {
        let mut properties = serde_json::Map::new();

        // Pattern property
        let mut pattern_prop = serde_json::Map::new();
        pattern_prop.insert(
            "description".to_string(),
            serde_json::json!("The regular expression pattern to search for in file contents"),
        );
        properties.insert(
            "pattern".to_string(),
            serde_json::Value::Object(pattern_prop),
        );

        // Path property
        let mut path_prop = serde_json::Map::new();
        path_prop.insert(
            "description".to_string(),
            serde_json::json!(
                "The directory to search in. Defaults to the current working directory."
            ),
        );
        properties.insert("path".to_string(), serde_json::Value::Object(path_prop));

        // Include property
        let mut include_prop = serde_json::Map::new();
        include_prop.insert(
            "description".to_string(),
            serde_json::json!(
                "File pattern to include in the search (e.g. \"*.js\", \"*.{ts,tsx}\")"
            ),
        );
        properties.insert(
            "include".to_string(),
            serde_json::Value::Object(include_prop),
        );

        Self {
            name: "GrepTool".to_string(),
            description: "Search file contents by regex pattern".to_string(),
            // tool_type: "custom".to_string(),
            input_schema: InputSchema {
                properties,
                schema_type: "object".to_string(),
                required: vec!["pattern".to_string()],
            },
        }
    }

    pub fn ls() -> Self {
        let mut properties = serde_json::Map::new();

        // Path property
        let mut path_prop = serde_json::Map::new();
        path_prop.insert(
            "description".to_string(),
            serde_json::json!(
                "The absolute path to the directory to list (must be absolute, not relative)"
            ),
        );
        properties.insert("path".to_string(), serde_json::Value::Object(path_prop));

        // Ignore property
        let mut ignore_prop = serde_json::Map::new();
        ignore_prop.insert(
            "description".to_string(),
            serde_json::json!("List of glob patterns to ignore"),
        );
        properties.insert("ignore".to_string(), serde_json::Value::Object(ignore_prop));

        Self {
            name: "LS".to_string(),
            description: "List files and directories in a given path".to_string(),
            // tool_type: "custom".to_string(),
            input_schema: InputSchema {
                properties,
                schema_type: "object".to_string(),
                required: vec!["path".to_string()],
            },
        }
    }

    pub fn view() -> Self {
        let mut properties = serde_json::Map::new();

        // File path property
        let mut file_path_prop = serde_json::Map::new();
        file_path_prop.insert(
            "description".to_string(),
            serde_json::json!("The absolute path to the file to read"),
        );
        properties.insert(
            "file_path".to_string(),
            serde_json::Value::Object(file_path_prop),
        );

        // Offset property
        let mut offset_prop = serde_json::Map::new();
        offset_prop.insert(
            "description".to_string(),
            serde_json::json!("The line number to start reading from. Only provide if the file is too large to read at once"),
        );
        properties.insert("offset".to_string(), serde_json::Value::Object(offset_prop));

        // Limit property
        let mut limit_prop = serde_json::Map::new();
        limit_prop.insert(
            "description".to_string(),
            serde_json::json!("The number of lines to read. Only provide if the file is too large to read at once."),
        );
        properties.insert("limit".to_string(), serde_json::Value::Object(limit_prop));

        Self {
            name: "View".to_string(),
            description: "Read a file from the local filesystem".to_string(),
            // tool_type: "custom".to_string(),
            input_schema: InputSchema {
                properties,
                schema_type: "object".to_string(),
                required: vec!["file_path".to_string()],
            },
        }
    }

    pub fn edit() -> Self {
        let mut properties = serde_json::Map::new();

        // File path property
        let mut file_path_prop = serde_json::Map::new();
        file_path_prop.insert(
            "description".to_string(),
            serde_json::json!("The absolute path to the file to modify"),
        );
        properties.insert(
            "file_path".to_string(),
            serde_json::Value::Object(file_path_prop),
        );

        // Old string property
        let mut old_string_prop = serde_json::Map::new();
        old_string_prop.insert(
            "description".to_string(),
            serde_json::json!("The text to replace"),
        );
        properties.insert(
            "old_string".to_string(),
            serde_json::Value::Object(old_string_prop),
        );

        // New string property
        let mut new_string_prop = serde_json::Map::new();
        new_string_prop.insert(
            "description".to_string(),
            serde_json::json!("The text to replace it with"),
        );
        properties.insert(
            "new_string".to_string(),
            serde_json::Value::Object(new_string_prop),
        );

        Self {
            name: "Edit".to_string(),
            description: "Edit a file in the local filesystem".to_string(),
            // tool_type: "custom".to_string(),
            input_schema: InputSchema {
                properties,
                schema_type: "object".to_string(),
                required: vec![
                    "file_path".to_string(),
                    "old_string".to_string(),
                    "new_string".to_string(),
                ],
            },
        }
    }

    pub fn replace() -> Self {
        let mut properties = serde_json::Map::new();

        // File path property
        let mut file_path_prop = serde_json::Map::new();
        file_path_prop.insert(
            "description".to_string(),
            serde_json::json!(
                "The absolute path to the file to write (must be absolute, not relative)"
            ),
        );
        properties.insert(
            "file_path".to_string(),
            serde_json::Value::Object(file_path_prop),
        );

        // Content property
        let mut content_prop = serde_json::Map::new();
        content_prop.insert(
            "description".to_string(),
            serde_json::json!("The content to write to the file"),
        );
        properties.insert(
            "content".to_string(),
            serde_json::Value::Object(content_prop),
        );

        Self {
            name: "Replace".to_string(),
            description: "Write a file to the local filesystem".to_string(),
            // tool_type: "custom".to_string(),
            input_schema: InputSchema {
                properties,
                schema_type: "object".to_string(),
                required: vec!["file_path".to_string(), "content".to_string()],
            },
        }
    }

    /// Get all available tools
    pub fn all() -> Vec<Self> {
        vec![
            Self::bash(),
            Self::glob_tool(),
            Self::grep_tool(),
            Self::ls(),
            Self::view(),
            Self::edit(),
            Self::replace(),
        ]
    }
}
