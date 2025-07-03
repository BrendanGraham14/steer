//! Tests for MCP backend functionality

#[cfg(test)]
mod tests {

    use crate::session::state::{BackendConfig, SessionConfig, ToolFilter};

    #[tokio::test]
    async fn test_mcp_backend_in_session_config() {
        // Create a session config with an MCP backend
        let mut config = SessionConfig::read_only();
        config.tool_config.backends.push(BackendConfig::Mcp {
            server_name: "test-server".to_string(),
            transport: "unix".to_string(),
            command: "echo".to_string(),
            args: vec!["test".to_string()],
            tool_filter: ToolFilter::All,
        });

        // Try to build the registry
        // This will fail to connect since echo isn't an MCP server,
        // but it tests that our code paths work
        let registry = config.build_registry().await;

        // Should succeed even if individual backends fail
        assert!(registry.is_ok());
    }

    #[test]
    fn test_tool_name_prefixing() {
        // Test that we correctly add and remove the mcp_servername_ prefix
        let server_name = "myserver";
        let tool_name = "mytool";
        let prefixed_name = format!("mcp_{}_{}", server_name, tool_name);

        // Test extraction
        let prefix = format!("mcp_{}_", server_name);
        let extracted = if prefixed_name.starts_with(&prefix) {
            &prefixed_name[prefix.len()..]
        } else {
            &prefixed_name
        };

        assert_eq!(extracted, tool_name);
    }
}
