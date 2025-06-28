use conductor_macros::tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::result::GlobResult;
use crate::{ExecutionContext, ToolError};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GlobParams {
    /// The glob pattern to match files against
    pub pattern: String,
    /// Optional directory to search in. Defaults to the current working directory.
    pub path: Option<String>,
}

tool! {
    GlobTool {
        params: GlobParams,
        output: GlobResult,
        variant: Glob,
        description: r#"Fast file pattern matching tool that works with any codebase size.
- Supports glob patterns like "**/*.js" or "src/**/*.ts"
- Returns matching file paths sorted by modification time
- Use this tool when you need to find files by name patterns"#,
        name: "glob",
        require_approval: false
    }

    async fn run(
        _tool: &GlobTool,
        params: GlobParams,
        context: &ExecutionContext,
    ) -> Result<GlobResult, ToolError> {
        if context.is_cancelled() {
            return Err(ToolError::Cancelled(GLOB_TOOL_NAME.to_string()));
        }

        let search_path = params.path.as_deref().unwrap_or(".");
        let base_path = if Path::new(search_path).is_absolute() {
            Path::new(search_path).to_path_buf()
        } else {
            context.working_directory.join(search_path)
        };

        let glob_pattern = if base_path.to_string_lossy() == "." {
            params.pattern.clone()
        } else {
            format!("{}/{}", base_path.display(), params.pattern)
        };

        let mut results = Vec::new();
        match glob::glob(&glob_pattern) {
            Ok(paths) => {
                for entry in paths {
                    if context.is_cancelled() {
                        return Err(ToolError::Cancelled(GLOB_TOOL_NAME.to_string()));
                    }

                    match entry {
                        Ok(path) => {
                            results.push(path.display().to_string());
                        }
                        Err(e) => {
                            return Err(ToolError::execution(
                                GLOB_TOOL_NAME,
                                format!("Error matching glob pattern '{}': {}", glob_pattern, e),
                            ));
                        }
                    }
                }
            }
            Err(e) => {
                return Err(ToolError::execution(
                    GLOB_TOOL_NAME,
                    format!("Invalid glob pattern '{}': {}", glob_pattern, e),
                ));
            }
        }

        // Sort results for consistent output
        results.sort();
        Ok(GlobResult {
            matches: results,
            pattern: params.pattern,
        })
    }
}
