use std::collections::HashMap;
use std::sync::Arc;

use crate::api::tools::Tool as ApiTool;
use crate::tools::traits::Tool as ToolTrait;

/// Builder for constructing a tool registry
pub struct ToolExecutorBuilder {
    registry: HashMap<String, Arc<dyn ToolTrait>>,
}

impl ToolExecutorBuilder {
    /// Create a new empty builder
    pub fn new() -> Self {
        Self {
            registry: HashMap::new(),
        }
    }

    /// Add a specific tool implementation to the registry
    pub fn add_tool<T: ToolTrait + Default + 'static>(mut self) -> Self {
        let tool: Arc<dyn ToolTrait> = Arc::new(T::default());
        self.registry.insert(tool.name().to_string(), tool);
        self
    }

    /// Add an already constructed tool to the registry
    pub fn with_tool(mut self, tool: Arc<dyn ToolTrait>) -> Self {
        self.registry.insert(tool.name().to_string(), tool);
        self
    }

    /// Build the ToolExecutor with the configured registry
    pub fn build(self) -> super::tool_executor::ToolExecutor {
        super::tool_executor::ToolExecutor {
            registry: Arc::new(self.registry),
        }
    }

    /// Create a builder pre-configured with all standard tools
    pub fn standard() -> Self {
        Self::new()
            .add_tool::<crate::tools::bash::BashTool>()
            .add_tool::<crate::tools::grep_tool::GrepTool>()
            .add_tool::<crate::tools::dispatch_agent::DispatchAgentTool>()
            .add_tool::<crate::tools::glob_tool::GlobTool>()
            .add_tool::<crate::tools::ls::LsTool>()
            .add_tool::<crate::tools::view::ViewTool>()
            .add_tool::<crate::tools::edit::EditTool>()
            .add_tool::<crate::tools::replace::ReplaceTool>()
            .add_tool::<crate::tools::fetch::FetchTool>()
    }

    /// Create a builder pre-configured with read-only tools
    pub fn read_only() -> Self {
        Self::new()
            .add_tool::<crate::tools::grep_tool::GrepTool>()
            .add_tool::<crate::tools::glob_tool::GlobTool>()
            .add_tool::<crate::tools::ls::LsTool>()
            .add_tool::<crate::tools::view::ViewTool>()
    }

    /// Convert registry tools to API tool descriptions
    pub fn to_api_tools(&self) -> Vec<ApiTool> {
        self.registry
            .values()
            .map(|tool| ApiTool {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                input_schema: tool.input_schema().clone(),
            })
            .collect()
    }
}
