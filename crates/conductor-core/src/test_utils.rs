//! Test utilities for conductor-core
//!
//! This module provides helpers for testing that need to be accessible
//! across crate boundaries.

use crate::app::AppConfig;
use crate::auth::{AuthStorage, CredentialType};
use crate::config::LlmConfigProvider;
use std::sync::Arc;

/// In-memory storage for testing - doesn't use keyring or filesystem
pub struct InMemoryAuthStorage {
    credentials:
        Arc<tokio::sync::Mutex<std::collections::HashMap<String, crate::auth::Credential>>>,
}

impl Default for InMemoryAuthStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryAuthStorage {
    pub fn new() -> Self {
        Self {
            credentials: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }
}

#[async_trait::async_trait]
impl AuthStorage for InMemoryAuthStorage {
    async fn get_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> crate::auth::Result<Option<crate::auth::Credential>> {
        let key = format!("{provider}-{credential_type}");
        Ok(self.credentials.lock().await.get(&key).cloned())
    }

    async fn set_credential(
        &self,
        provider: &str,
        credential: crate::auth::Credential,
    ) -> crate::auth::Result<()> {
        let key = format!("{}-{}", provider, credential.credential_type());
        self.credentials.lock().await.insert(key, credential);
        Ok(())
    }

    async fn remove_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> crate::auth::Result<()> {
        let key = format!("{provider}-{credential_type}");
        self.credentials.lock().await.remove(&key);
        Ok(())
    }
}

/// Create an `LlmConfigProvider` backed by in-memory auth storage for tests
pub fn test_llm_config_provider() -> LlmConfigProvider {
    let storage = Arc::new(InMemoryAuthStorage::new());
    LlmConfigProvider::new(storage)
}

/// Convenience to build an `AppConfig` for tests with a fresh provider
pub fn test_app_config() -> AppConfig {
    AppConfig {
        llm_config_provider: test_llm_config_provider(),
    }
}
