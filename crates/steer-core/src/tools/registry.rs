use std::collections::HashMap;
use std::sync::Arc;

use steer_tools::ToolSchema;

use super::backend::ToolBackend;
use super::capability::Capabilities;
use super::mcp::McpBackend;
use super::static_tool::StaticToolErased;

pub struct ToolRegistry {
    static_tools: HashMap<String, Box<dyn StaticToolErased>>,
    mcp_backends: Vec<Arc<McpBackend>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            static_tools: HashMap::new(),
            mcp_backends: Vec::new(),
        }
    }

    pub fn register_static<T: StaticToolErased + 'static>(&mut self, tool: T) {
        self.static_tools
            .insert(tool.name().to_string(), Box::new(tool));
    }

    pub fn register_mcp(&mut self, backend: Arc<McpBackend>) {
        self.mcp_backends.push(backend);
    }

    pub async fn available_schemas(&self, available_caps: Capabilities) -> Vec<ToolSchema> {
        let mut schemas = Vec::new();

        for tool in self.static_tools.values() {
            if available_caps.satisfies(tool.required_capabilities()) {
                schemas.push(tool.schema());
            }
        }

        for backend in &self.mcp_backends {
            schemas.extend(backend.get_tool_schemas().await);
        }

        schemas
    }

    pub fn static_tool(&self, name: &str) -> Option<&dyn StaticToolErased> {
        self.static_tools.get(name).map(|b| b.as_ref())
    }

    pub fn find_mcp_backend(&self, tool_name: &str) -> Option<&Arc<McpBackend>> {
        self.mcp_backends
            .iter()
            .find(|&backend| backend.has_tool(tool_name))
    }

    pub fn is_static_tool(&self, name: &str) -> bool {
        self.static_tools.contains_key(name)
    }

    pub fn static_tool_names(&self) -> Vec<&str> {
        self.static_tools.keys().map(|s| s.as_str()).collect()
    }

    pub fn requires_approval(&self, tool_name: &str) -> bool {
        if let Some(tool) = self.static_tools.get(tool_name) {
            return tool.requires_approval();
        }
        true
    }

    pub fn required_capabilities(&self, tool_name: &str) -> Option<Capabilities> {
        self.static_tools
            .get(tool_name)
            .map(|t| t.required_capabilities())
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::capability::Capabilities;
    use crate::tools::static_tool::{StaticToolContext, StaticToolError};
    use async_trait::async_trait;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, JsonSchema)]
    struct TestParams {
        value: String,
    }

    #[derive(Debug, Serialize)]
    struct TestOutput {
        result: String,
    }

    struct TestTool;

    #[async_trait]
    impl super::super::static_tool::StaticTool for TestTool {
        type Params = TestParams;
        type Output = TestOutput;

        const NAME: &'static str = "test_tool";
        const DESCRIPTION: &'static str = "A test tool";
        const REQUIRES_APPROVAL: bool = false;
        const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

        async fn execute(
            &self,
            params: Self::Params,
            _ctx: &StaticToolContext,
        ) -> Result<Self::Output, StaticToolError> {
            Ok(TestOutput {
                result: params.value,
            })
        }
    }

    struct AgentTool;

    #[async_trait]
    impl super::super::static_tool::StaticTool for AgentTool {
        type Params = TestParams;
        type Output = TestOutput;

        const NAME: &'static str = "agent_tool";
        const DESCRIPTION: &'static str = "Needs agent spawner";
        const REQUIRES_APPROVAL: bool = false;
        const REQUIRED_CAPABILITIES: Capabilities = Capabilities::AGENT;

        async fn execute(
            &self,
            params: Self::Params,
            _ctx: &StaticToolContext,
        ) -> Result<Self::Output, StaticToolError> {
            Ok(TestOutput {
                result: params.value,
            })
        }
    }

    #[tokio::test]
    async fn test_capability_filtering() {
        let mut registry = ToolRegistry::new();
        registry.register_static(TestTool);
        registry.register_static(AgentTool);

        let schemas = registry.available_schemas(Capabilities::WORKSPACE).await;
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "test_tool");

        let schemas = registry.available_schemas(Capabilities::AGENT).await;
        assert_eq!(schemas.len(), 2);
    }

    #[test]
    fn test_requires_approval() {
        let mut registry = ToolRegistry::new();
        registry.register_static(TestTool);

        assert!(!registry.requires_approval("test_tool"));
        assert!(registry.requires_approval("unknown_tool"));
    }
}
