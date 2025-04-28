use anyhow::Result;
use glob::glob;
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::Path;
use tokio_util::sync::CancellationToken;

use crate::tools::ToolError;
use coder_macros::tool;

#[derive(Deserialize, Debug, JsonSchema)]
struct GlobParams {
    /// The glob pattern to match files against
    pattern: String,
    /// Optional directory to search in. Defaults to the current working directory.
    path: Option<String>,
}

tool! {
    GlobTool {
        params: GlobParams,
        description: r#"- Fast file pattern matching tool that works with any codebase size
- Supports glob patterns like \"**/*.js\" or \"src/**/*.ts\"
- Returns matching file paths sorted by modification time
- Use this tool when you need to find files by name patterns
- When you are doing an open ended search that may require multiple rounds of globbing and grepping, use the Agent tool instead"#,
        name: "glob"
    }

    async fn run(
        _tool: &GlobTool,
        params: GlobParams,
        token: Option<CancellationToken>,
    ) -> Result<String, ToolError> {
        if let Some(t) = &token {
            if t.is_cancelled() {
                return Err(ToolError::Cancelled("GlobTool".to_string()));
            }
        }

        glob_search_internal(&params.pattern, params.path.as_deref().unwrap_or("."))
            .map_err(|e| ToolError::execution("GlobTool", e))
    }
}

fn glob_search_internal(pattern: &str, path: &str) -> Result<String> {
    let base_path = Path::new(path);
    let glob_pattern = if base_path.to_string_lossy() == "." {
        pattern.to_string()
    } else {
        format!("{}/{}", base_path.display(), pattern)
    };

    let mut results = Vec::new();
    for entry in glob(&glob_pattern)? {
        match entry {
            Ok(path) => {
                results.push(path.display().to_string());
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Error matching glob pattern '{}': {}",
                    glob_pattern,
                    e
                ));
            }
        }
    }

    if results.is_empty() {
        Ok("No files found matching the pattern.".to_string())
    } else {
        Ok(results.join("\n"))
    }
}
