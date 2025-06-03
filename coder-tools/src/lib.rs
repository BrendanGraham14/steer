pub mod context;
pub mod error;
pub mod schema;
pub mod tools;
pub mod traits;

pub use context::ExecutionContext;
pub use error::ToolError;
pub use schema::{InputSchema, ToolCall, ToolResult};
pub use traits::Tool;

#[cfg(test)]
mod description_format_tests {
    use crate::*;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
    pub struct SimpleParams {
        pub value: String,
    }

    mod string_literal_tool {
        use super::*;
        use coder_macros::tool;

        // Example 1: String literal description (traditional way)
        tool! {
            StringLiteralTool {
                params: SimpleParams,
                description: "This is a simple string literal description",
                name: "string_literal_tool",
                require_approval: false
            }

            async fn run(
                _tool: &StringLiteralTool,
                params: SimpleParams,
                _context: &ExecutionContext,
            ) -> Result<String, ToolError> {
                Ok(format!("Processed: {}", params.value))
            }
        }
    }

    mod formatted_string_tool {
        use super::*;
        use coder_macros::tool;

        // Example 2: Formatted string description (new capability)
        tool! {
            FormattedStringTool {
                params: SimpleParams,
                description: format!("This is a formatted description with version {} and features: {:?}",
                                   "1.0.0",
                                   vec!["advanced", "dynamic"]),
                name: "formatted_string_tool",
                require_approval: false
            }

            async fn run(
                _tool: &FormattedStringTool,
                params: SimpleParams,
                _context: &ExecutionContext,
            ) -> Result<String, ToolError> {
                Ok(format!("Formatted result: {}", params.value))
            }
        }
    }

    mod constant_description_tool {
        use super::*;
        use coder_macros::tool;

        // Example 3: Using a constant in the description
        const TOOL_VERSION: &str = "2.1.0";

        tool! {
            ConstantDescriptionTool {
                params: SimpleParams,
                description: format!("Tool version {} with enhanced capabilities", TOOL_VERSION),
                name: "constant_description_tool",
                require_approval: false
            }

            async fn run(
                _tool: &ConstantDescriptionTool,
                params: SimpleParams,
                _context: &ExecutionContext,
            ) -> Result<String, ToolError> {
                Ok(format!("Version {} result: {}", TOOL_VERSION, params.value))
            }
        }
    }

    #[test]
    fn test_string_literal_description() {
        use string_literal_tool::*;
        let tool = StringLiteralTool::default();
        assert_eq!(
            tool.description(),
            "This is a simple string literal description"
        );
    }

    #[test]
    fn test_formatted_string_description() {
        use formatted_string_tool::*;
        let tool = FormattedStringTool::default();
        let desc = tool.description();
        assert!(desc.contains("version 1.0.0"));
        assert!(desc.contains("advanced"));
        assert!(desc.contains("dynamic"));
    }

    #[test]
    fn test_constant_description() {
        use constant_description_tool::*;
        let tool = ConstantDescriptionTool::default();
        let desc = tool.description();
        assert!(desc.contains("Tool version 2.1.0"));
        assert!(desc.contains("enhanced capabilities"));
    }
}
