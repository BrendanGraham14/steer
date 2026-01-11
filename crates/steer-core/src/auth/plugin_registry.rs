use crate::config::provider::ProviderId;
use crate::error::{Error, Result};
use std::collections::HashMap;
use std::sync::Arc;
use steer_auth_anthropic::AnthropicAuthPlugin;
use steer_auth_openai::OpenAiAuthPlugin;
use steer_auth_plugin::identifiers::ProviderId as PluginProviderId;
use steer_auth_plugin::plugin::AuthPlugin;

#[derive(Clone, Default)]
pub struct AuthPluginRegistry {
    plugins: HashMap<ProviderId, Arc<dyn AuthPlugin>>,
}

impl AuthPluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_defaults() -> Result<Self> {
        let mut registry = Self::new();
        registry.register(Arc::new(OpenAiAuthPlugin::new()))?;
        registry.register(Arc::new(AnthropicAuthPlugin::new()))?;
        Ok(registry)
    }

    pub fn register(&mut self, plugin: Arc<dyn AuthPlugin>) -> Result<()> {
        let plugin_id: PluginProviderId = plugin.provider_id();
        let provider_id = ProviderId(plugin_id.0);
        if self.plugins.contains_key(&provider_id) {
            return Err(Error::Configuration(format!(
                "Auth plugin conflict for provider {}",
                provider_id.as_str()
            )));
        }
        self.plugins.insert(provider_id, plugin);
        Ok(())
    }

    pub fn get(&self, provider_id: &ProviderId) -> Option<&Arc<dyn AuthPlugin>> {
        self.plugins.get(provider_id)
    }

    pub fn all(&self) -> impl Iterator<Item = &Arc<dyn AuthPlugin>> {
        self.plugins.values()
    }
}
