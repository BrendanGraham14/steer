use std::collections::HashMap;
use std::sync::Arc;

use steer_tools::ToolSchema;

use super::backend::ToolBackend;
use super::builtin_tool::BuiltinToolErased;
use super::capability::Capabilities;
use super::mcp::McpBackend;

pub struct ToolRegistry {
    builtin_tools: HashMap<String, Box<dyn BuiltinToolErased>>,
    mcp_backends: Vec<Arc<McpBackend>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            builtin_tools: HashMap::new(),
            mcp_backends: Vec::new(),
        }
    }

    pub fn register_builtin<T: BuiltinToolErased + 'static>(&mut self, tool: T) {
        self.builtin_tools
            .insert(tool.name().to_string(), Box::new(tool));
    }

    pub fn register_mcp(&mut self, backend: Arc<McpBackend>) {
        self.mcp_backends.push(backend);
    }

    pub async fn available_schemas(&self, available_caps: Capabilities) -> Vec<ToolSchema> {
        let mut schemas = Vec::new();

        for tool in self.builtin_tools.values() {
            if available_caps.satisfies(tool.required_capabilities()) {
                schemas.push(tool.schema());
            }
        }

        for backend in &self.mcp_backends {
            schemas.extend(backend.get_tool_schemas().await);
        }

        schemas
    }

    pub fn builtin_tool(&self, name: &str) -> Option<&dyn BuiltinToolErased> {
        self.builtin_tools.get(name).map(|b| b.as_ref())
    }

    pub fn find_mcp_backend(&self, tool_name: &str) -> Option<&Arc<McpBackend>> {
        self.mcp_backends
            .iter()
            .find(|&backend| backend.has_tool(tool_name))
    }

    pub fn is_builtin_tool(&self, name: &str) -> bool {
        self.builtin_tools.contains_key(name)
    }

    pub fn builtin_tool_names(&self) -> Vec<&str> {
        self.builtin_tools.keys().map(|s| s.as_str()).collect()
    }

    pub fn requires_approval(&self, tool_name: &str) -> bool {
        if let Some(tool) = self.builtin_tools.get(tool_name) {
            return tool.requires_approval();
        }
        true
    }

    pub fn required_capabilities(&self, tool_name: &str) -> Option<Capabilities> {
        self.builtin_tools
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
    use crate::tools::builtin_tool::{BuiltinToolContext, BuiltinToolError};
    use crate::tools::capability::Capabilities;
    use async_trait::async_trait;
    use schemars::JsonSchema;
    use serde::Deserialize;
    use steer_tools::ToolSpec;
    use steer_tools::error::ToolExecutionError;

    #[derive(Debug, Deserialize, JsonSchema)]
    struct TestParams {
        value: String,
    }

    #[derive(Debug)]
    struct TestOutput {
        result: String,
    }

    impl From<TestOutput> for steer_tools::result::ToolResult {
        fn from(output: TestOutput) -> Self {
            steer_tools::result::ToolResult::External(steer_tools::result::ExternalResult {
                tool_name: "test_tool".to_string(),
                payload: output.result,
            })
        }
    }

    struct TestTool;

    #[derive(Debug, Clone, thiserror::Error)]
    #[error("test tool error: {message}")]
    struct TestToolError {
        message: String,
    }

    struct TestToolSpec;

    impl ToolSpec for TestToolSpec {
        type Params = TestParams;
        type Result = TestOutput;
        type Error = TestToolError;

        const NAME: &'static str = "test_tool";
        const DISPLAY_NAME: &'static str = "Test Tool";

        fn execution_error(error: Self::Error) -> ToolExecutionError {
            ToolExecutionError::External {
                tool_name: Self::NAME.to_string(),
                message: error.to_string(),
            }
        }
    }

    #[async_trait]
    impl super::super::builtin_tool::BuiltinTool for TestTool {
        type Params = TestParams;
        type Output = TestOutput;
        type Spec = TestToolSpec;

        const DESCRIPTION: &'static str = "A test tool";
        const REQUIRES_APPROVAL: bool = false;
        const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

        async fn execute(
            &self,
            params: Self::Params,
            _ctx: &BuiltinToolContext,
        ) -> Result<Self::Output, BuiltinToolError<TestToolError>> {
            Ok(TestOutput {
                result: params.value,
            })
        }
    }

    struct AgentTool;

    struct AgentToolSpec;

    impl ToolSpec for AgentToolSpec {
        type Params = TestParams;
        type Result = TestOutput;
        type Error = TestToolError;

        const NAME: &'static str = "agent_tool";
        const DISPLAY_NAME: &'static str = "Agent Tool";

        fn execution_error(error: Self::Error) -> ToolExecutionError {
            ToolExecutionError::External {
                tool_name: Self::NAME.to_string(),
                message: error.to_string(),
            }
        }
    }

    #[async_trait]
    impl super::super::builtin_tool::BuiltinTool for AgentTool {
        type Params = TestParams;
        type Output = TestOutput;
        type Spec = AgentToolSpec;

        const DESCRIPTION: &'static str = "Needs agent spawner";
        const REQUIRES_APPROVAL: bool = false;
        const REQUIRED_CAPABILITIES: Capabilities = Capabilities::AGENT;

        async fn execute(
            &self,
            params: Self::Params,
            _ctx: &BuiltinToolContext,
        ) -> Result<Self::Output, BuiltinToolError<TestToolError>> {
            Ok(TestOutput {
                result: params.value,
            })
        }
    }

    #[tokio::test]
    async fn test_capability_filtering() {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(TestTool);
        registry.register_builtin(AgentTool);

        let schemas = registry.available_schemas(Capabilities::WORKSPACE).await;
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "test_tool");

        let schemas = registry.available_schemas(Capabilities::AGENT).await;
        assert_eq!(schemas.len(), 2);
    }

    #[test]
    fn test_requires_approval() {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(TestTool);

        assert!(!registry.requires_approval("test_tool"));
        assert!(registry.requires_approval("unknown_tool"));
    }
}
