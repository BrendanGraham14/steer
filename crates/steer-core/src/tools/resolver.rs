use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use steer_tools::ToolSchema;
use tokio::sync::RwLock;

use super::backend::ToolBackend;
use super::mcp::McpBackend;

#[async_trait]
pub trait BackendResolver: Send + Sync {
    async fn resolve(&self, tool_name: &str) -> Option<Arc<dyn ToolBackend>>;

    async fn get_tool_schemas(&self) -> Vec<ToolSchema>;

    fn requires_approval(&self, tool_name: &str) -> Option<bool>;
}

#[async_trait]
impl BackendResolver for super::BackendRegistry {
    async fn resolve(&self, tool_name: &str) -> Option<Arc<dyn ToolBackend>> {
        self.get_backend_for_tool(tool_name).cloned()
    }

    async fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        let mut schemas = Vec::new();
        for (_, backend) in self.backends() {
            schemas.extend(backend.get_tool_schemas().await);
        }
        schemas
    }

    fn requires_approval(&self, _tool_name: &str) -> Option<bool> {
        None
    }
}

pub struct SessionMcpBackends {
    backends: RwLock<HashMap<String, Arc<McpBackend>>>,
    tool_to_backend: RwLock<HashMap<String, String>>,
    generations: RwLock<HashMap<String, u64>>,
}

impl SessionMcpBackends {
    pub fn new() -> Self {
        Self {
            backends: RwLock::new(HashMap::new()),
            tool_to_backend: RwLock::new(HashMap::new()),
            generations: RwLock::new(HashMap::new()),
        }
    }

    pub async fn next_generation(&self, server_name: &str) -> u64 {
        let mut generations = self.generations.write().await;
        let next = generations
            .get(server_name)
            .copied()
            .unwrap_or(0)
            .wrapping_add(1);
        generations.insert(server_name.to_string(), next);
        next
    }

    pub async fn is_current_generation(&self, server_name: &str, generation: u64) -> bool {
        let generations = self.generations.read().await;
        generations.get(server_name).copied().unwrap_or(0) == generation
    }

    pub async fn register(&self, server_name: String, backend: Arc<McpBackend>) {
        let tool_names = backend.supported_tools().await;

        let mut tool_mapping = self.tool_to_backend.write().await;
        tool_mapping.retain(|_, name| name != &server_name);
        for tool_name in tool_names {
            tool_mapping.insert(tool_name, server_name.clone());
        }
        drop(tool_mapping);

        let mut backends = self.backends.write().await;
        backends.insert(server_name, backend);
    }

    pub async fn unregister(&self, server_name: &str) -> Option<Arc<McpBackend>> {
        let mut backends = self.backends.write().await;
        let removed = backends.remove(server_name);

        if removed.is_some() {
            let mut tool_mapping = self.tool_to_backend.write().await;
            tool_mapping.retain(|_, name| name != server_name);
        }

        removed
    }

    pub async fn get(&self, server_name: &str) -> Option<Arc<McpBackend>> {
        let backends = self.backends.read().await;
        backends.get(server_name).cloned()
    }

    pub async fn clear(&self) {
        let mut backends = self.backends.write().await;
        backends.clear();
        drop(backends);

        let mut tool_mapping = self.tool_to_backend.write().await;
        tool_mapping.clear();

        let mut generations = self.generations.write().await;
        generations.clear();
    }
}

#[async_trait]
impl BackendResolver for SessionMcpBackends {
    async fn resolve(&self, tool_name: &str) -> Option<Arc<dyn ToolBackend>> {
        let tool_mapping = self.tool_to_backend.read().await;
        let server_name = tool_mapping.get(tool_name)?.clone();
        drop(tool_mapping);

        let backends = self.backends.read().await;
        backends
            .get(&server_name)
            .map(|b| b.clone() as Arc<dyn ToolBackend>)
    }

    async fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        let mut schemas = Vec::new();
        let backends = self.backends.read().await;
        for backend in backends.values() {
            schemas.extend(backend.get_tool_schemas().await);
        }
        schemas
    }

    fn requires_approval(&self, tool_name: &str) -> Option<bool> {
        let tool_mapping = self.tool_to_backend.try_read().ok()?;
        if tool_mapping.contains_key(tool_name) {
            Some(true)
        } else {
            None
        }
    }
}

pub struct OverlayResolver {
    session: Arc<SessionMcpBackends>,
    static_resolver: Arc<dyn BackendResolver>,
}

impl OverlayResolver {
    pub fn new(
        session: Arc<SessionMcpBackends>,
        static_resolver: Arc<dyn BackendResolver>,
    ) -> Self {
        Self {
            session,
            static_resolver,
        }
    }

    pub fn session_backends(&self) -> &Arc<SessionMcpBackends> {
        &self.session
    }
}

#[async_trait]
impl BackendResolver for OverlayResolver {
    async fn resolve(&self, tool_name: &str) -> Option<Arc<dyn ToolBackend>> {
        if let Some(backend) = self.session.resolve(tool_name).await {
            return Some(backend);
        }
        self.static_resolver.resolve(tool_name).await
    }

    async fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        let mut schemas = self.session.get_tool_schemas().await;
        schemas.extend(self.static_resolver.get_tool_schemas().await);
        schemas
    }

    fn requires_approval(&self, tool_name: &str) -> Option<bool> {
        self.session
            .requires_approval(tool_name)
            .or_else(|| self.static_resolver.requires_approval(tool_name))
    }
}
