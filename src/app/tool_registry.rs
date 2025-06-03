use std::collections::HashMap;
use std::sync::Arc;

use crate::api::tools::Tool as ApiTool;
use crate::tools::traits::Tool as ToolTrait;
use crate::tools::{BackendRegistry, LocalBackend};

/// Builder for constructing a tool registry
pub struct ToolExecutorBuilder {
    registry: HashMap<String, Arc<dyn ToolTrait>>,
}

impl Default for ToolExecutorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolExecutorBuilder {
    pub fn new() -> Self {
        Self {
            registry: HashMap::new(),
        }
    }

    pub fn add_tool<T: ToolTrait + Default + 'static>(mut self) -> Self {
        let tool: Arc<dyn ToolTrait> = Arc::new(T::default());
        self.registry.insert(tool.name().to_string(), tool);
        self
    }

    pub fn with_tool(mut self, tool: Arc<dyn ToolTrait>) -> Self {
        self.registry.insert(tool.name().to_string(), tool);
        self
    }

    pub fn build(self) -> super::tool_executor::ToolExecutor {
        // Create LocalBackend with the same registry
        let local_backend = Arc::new(LocalBackend::new(Arc::new(self.registry.clone())));

        // Create backend registry and register the local backend
        let mut backend_registry = BackendRegistry::new();
        backend_registry.register("local".to_string(), local_backend);

        super::tool_executor::ToolExecutor {
            registry: Arc::new(self.registry),
            backend_registry: Some(Arc::new(backend_registry)),
        }
    }

    pub fn standard() -> Self {
        Self::new()
            // For now, just use the remaining tools that haven't been migrated yet
            .add_tool::<crate::tools::dispatch_agent::DispatchAgentTool>()
            .add_tool::<crate::tools::edit::multi_edit::MultiEditTool>()
            .add_tool::<crate::tools::replace::ReplaceTool>()
            .add_tool::<crate::tools::fetch::FetchTool>()
            .add_tool::<crate::tools::todo::read::TodoReadTool>()
            .add_tool::<crate::tools::todo::write::TodoWriteTool>()
    }

    /// Create a builder pre-configured with read-only tools
    pub fn read_only() -> Self {
        Self::new()
            .add_tool::<crate::tools::todo::read::TodoReadTool>()
            .add_tool::<crate::tools::todo::write::TodoWriteTool>()
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
