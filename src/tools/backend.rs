use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::api::ToolCall;
use crate::tools::{ToolError, ExecutionContext};

/// Metadata about a tool backend for debugging and monitoring
#[derive(Debug, Clone)]
pub struct BackendMetadata {
    pub name: String,
    pub backend_type: String,
    pub location: Option<String>,
    pub additional_info: HashMap<String, String>,
}

impl Default for BackendMetadata {
    fn default() -> Self {
        Self {
            name: "Unknown".to_string(),
            backend_type: "Unknown".to_string(),
            location: None,
            additional_info: HashMap::new(),
        }
    }
}

impl BackendMetadata {
    pub fn new(name: String, backend_type: String) -> Self {
        Self {
            name,
            backend_type,
            location: None,
            additional_info: HashMap::new(),
        }
    }

    pub fn with_location(mut self, location: String) -> Self {
        self.location = Some(location);
        self
    }

    pub fn with_info(mut self, key: String, value: String) -> Self {
        self.additional_info.insert(key, value);
        self
    }
}

/// Simple trait for tool execution backends
/// 
/// This trait abstracts different execution environments for tools,
/// allowing tools to run locally, on remote machines, in containers,
/// or through proxy services.
#[async_trait]
pub trait ToolBackend: Send + Sync {
    /// Execute a tool call in this backend's environment
    /// 
    /// # Arguments
    /// * `tool_call` - The tool call containing name, parameters, and ID
    /// * `context` - Execution context with session info, cancellation, etc.
    /// 
    /// # Returns
    /// The string output of the tool on success, or a ToolError on failure
    async fn execute(
        &self,
        tool_call: &ToolCall,
        context: &ExecutionContext,
    ) -> Result<String, ToolError>;
    
    /// List the tools this backend can handle
    /// 
    /// Returns a vector of tool names that this backend supports.
    /// The backend registry uses this to map tools to backends.
    fn supported_tools(&self) -> Vec<&'static str>;
    
    /// Backend metadata for debugging and monitoring
    /// 
    /// Override this method to provide additional information about
    /// the backend for observability and troubleshooting.
    fn metadata(&self) -> BackendMetadata {
        BackendMetadata::default()
    }
    
    /// Check if the backend is healthy and ready to execute tools
    /// 
    /// This method can be used for health checks and load balancing.
    /// Default implementation returns true.
    async fn health_check(&self) -> bool {
        true
    }
}

/// Registry that maps tool names to their backends
/// 
/// When a backend is registered, the registry queries its supported_tools()
/// and creates mappings for each tool. This allows for efficient lookup
/// of the appropriate backend for a given tool name.
pub struct BackendRegistry {
    backends: Vec<(String, Arc<dyn ToolBackend>)>,
    tool_mapping: HashMap<String, Arc<dyn ToolBackend>>,
}

impl BackendRegistry {
    /// Create a new empty backend registry
    pub fn new() -> Self {
        Self {
            backends: Vec::new(),
            tool_mapping: HashMap::new(),
        }
    }

    /// Register a backend with the given name
    /// 
    /// This method queries the backend's supported_tools() and creates
    /// mappings for each tool. If a tool is already mapped to another
    /// backend, it will be overwritten.
    /// 
    /// # Arguments
    /// * `name` - A unique name for this backend instance
    /// * `backend` - The backend implementation
    pub fn register(&mut self, name: String, backend: Arc<dyn ToolBackend>) {
        // Map each tool this backend supports
        for tool_name in backend.supported_tools() {
            self.tool_mapping.insert(tool_name.to_string(), backend.clone());
        }
        self.backends.push((name, backend));
    }
    
    /// Get the backend for a specific tool
    /// 
    /// Returns the backend that can handle the given tool name,
    /// or None if no backend supports that tool.
    /// 
    /// # Arguments
    /// * `tool_name` - The name of the tool to look up
    pub fn get_backend_for_tool(&self, tool_name: &str) -> Option<&Arc<dyn ToolBackend>> {
        self.tool_mapping.get(tool_name)
    }

    /// Get all registered backends
    /// 
    /// Returns a vector of (name, backend) pairs for all registered backends.
    pub fn backends(&self) -> &Vec<(String, Arc<dyn ToolBackend>)> {
        &self.backends
    }

    /// Get all tool mappings
    /// 
    /// Returns a reference to the tool name -> backend mapping.
    pub fn tool_mappings(&self) -> &HashMap<String, Arc<dyn ToolBackend>> {
        &self.tool_mapping
    }

    /// Check which tools are supported
    /// 
    /// Returns a vector of all tool names that have registered backends.
    pub fn supported_tools(&self) -> Vec<String> {
        self.tool_mapping.keys().cloned().collect()
    }

    /// Remove a backend by name
    /// 
    /// This removes the backend and all its tool mappings.
    /// Returns true if a backend was removed, false if the name wasn't found.
    pub fn unregister(&mut self, name: &str) -> bool {
        if let Some(pos) = self.backends.iter().position(|(n, _)| n == name) {
            let (_, backend) = self.backends.remove(pos);
            
            // Remove all tool mappings for this backend
            self.tool_mapping.retain(|_tool, mapped_backend| {
                !Arc::ptr_eq(mapped_backend, &backend)
            });
            
            true
        } else {
            false
        }
    }

    /// Clear all backends and mappings
    pub fn clear(&mut self) {
        self.backends.clear();
        self.tool_mapping.clear();
    }
}

impl Default for BackendRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ToolCall;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    struct MockBackend {
        name: String,
        tools: Vec<&'static str>,
    }

    #[async_trait]
    impl ToolBackend for MockBackend {
        async fn execute(
            &self,
            tool_call: &ToolCall,
            _context: &ExecutionContext,
        ) -> Result<String, ToolError> {
            Ok(format!("Mock execution of {} by {}", tool_call.name, self.name))
        }

        fn supported_tools(&self) -> Vec<&'static str> {
            self.tools.clone()
        }

        fn metadata(&self) -> BackendMetadata {
            BackendMetadata::new(self.name.clone(), "Mock".to_string())
        }
    }

    #[tokio::test]
    async fn test_backend_registry() {
        let mut registry = BackendRegistry::new();

        let backend1 = Arc::new(MockBackend {
            name: "backend1".to_string(),
            tools: vec!["tool1", "tool2"],
        });

        let backend2 = Arc::new(MockBackend {
            name: "backend2".to_string(),
            tools: vec!["tool3", "tool4"],
        });

        registry.register("backend1".to_string(), backend1.clone());
        registry.register("backend2".to_string(), backend2.clone());

        // Test tool mappings
        assert!(registry.get_backend_for_tool("tool1").is_some());
        assert!(registry.get_backend_for_tool("tool3").is_some());
        assert!(registry.get_backend_for_tool("unknown_tool").is_none());

        // Test supported tools
        let supported = registry.supported_tools();
        assert_eq!(supported.len(), 4);
        assert!(supported.contains(&"tool1".to_string()));
        assert!(supported.contains(&"tool4".to_string()));

        // Test backend removal
        assert!(registry.unregister("backend1"));
        assert!(!registry.unregister("nonexistent"));
        
        // tool1 and tool2 should no longer be mapped
        assert!(registry.get_backend_for_tool("tool1").is_none());
        assert!(registry.get_backend_for_tool("tool3").is_some());
    }

    #[tokio::test]
    async fn test_mock_backend_execution() {
        let backend = MockBackend {
            name: "test".to_string(),
            tools: vec!["test_tool"],
        };

        let tool_call = ToolCall {
            name: "test_tool".to_string(),
            parameters: json!({}),
            id: "test_id".to_string(),
        };

        let context = ExecutionContext::new(
            "session".to_string(),
            "operation".to_string(),
            "tool_call".to_string(),
            CancellationToken::new(),
        );

        let result = backend.execute(&tool_call, &context).await.unwrap();
        assert!(result.contains("Mock execution"));
        assert!(result.contains("test_tool"));
        assert!(result.contains("test"));
    }
}