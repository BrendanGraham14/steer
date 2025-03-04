use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool definition that Claude can use
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
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
        Self {
            name: "Bash".to_string(),
            description: "Run a bash command in the terminal".to_string(),
            parameters: serde_json::json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to execute"
                    },
                    "timeout": {
                        "type": "number",
                        "description": "Optional timeout in milliseconds (max 600000)"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    pub fn glob_tool() -> Self {
        Self {
            name: "GlobTool".to_string(),
            description: "Find files by glob pattern".to_string(),
            parameters: serde_json::json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "The glob pattern to match files against"
                    },
                    "path": {
                        "type": "string",
                        "description": "The directory to search in. Defaults to the current working directory."
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    pub fn grep_tool() -> Self {
        Self {
            name: "GrepTool".to_string(),
            description: "Search file contents by regex pattern".to_string(),
            parameters: serde_json::json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "The regular expression pattern to search for in file contents"
                    },
                    "include": {
                        "type": "string",
                        "description": "File pattern to include in the search (e.g. \"*.js\", \"*.{ts,tsx}\")"
                    },
                    "path": {
                        "type": "string",
                        "description": "The directory to search in. Defaults to the current working directory."
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    pub fn ls() -> Self {
        Self {
            name: "LS".to_string(),
            description: "List files and directories in a given path".to_string(),
            parameters: serde_json::json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The absolute path to the directory to list (must be absolute, not relative)"
                    },
                    "ignore": {
                        "type": "array",
                        "description": "List of glob patterns to ignore",
                        "items": {
                            "type": "string"
                        }
                    }
                },
                "required": ["path"]
            }),
        }
    }

    pub fn view() -> Self {
        Self {
            name: "View".to_string(),
            description: "Read a file from the local filesystem".to_string(),
            parameters: serde_json::json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "The absolute path to the file to read"
                    },
                    "offset": {
                        "type": "number",
                        "description": "The line number to start reading from. Only provide if the file is too large to read at once"
                    },
                    "limit": {
                        "type": "number",
                        "description": "The number of lines to read. Only provide if the file is too large to read at once."
                    }
                },
                "required": ["file_path"]
            }),
        }
    }

    pub fn edit() -> Self {
        Self {
            name: "Edit".to_string(),
            description: "Edit a file in the local filesystem".to_string(),
            parameters: serde_json::json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "The absolute path to the file to modify"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The text to replace"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The text to replace it with"
                    }
                },
                "required": ["file_path", "old_string", "new_string"]
            }),
        }
    }

    pub fn replace() -> Self {
        Self {
            name: "Replace".to_string(),
            description: "Write a file to the local filesystem".to_string(),
            parameters: serde_json::json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "The absolute path to the file to write (must be absolute, not relative)"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write to the file"
                    }
                },
                "required": ["file_path", "content"]
            }),
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