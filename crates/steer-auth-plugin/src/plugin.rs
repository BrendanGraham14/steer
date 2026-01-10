use crate::directive::AuthDirective;
use crate::error::AuthError;
use crate::flow::{AuthMethod, DynAuthenticationFlow};
use crate::identifiers::{ModelId, ProviderId};
use crate::storage::AuthStorage;
use crate::strategy::AuthSource;
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait AuthPlugin: Send + Sync {
    fn provider_id(&self) -> ProviderId;
    fn supported_methods(&self) -> Vec<AuthMethod>;

    fn create_flow(&self, storage: Arc<dyn AuthStorage>) -> Option<Box<dyn DynAuthenticationFlow>>;

    async fn resolve_auth(
        &self,
        storage: Arc<dyn AuthStorage>,
    ) -> Result<Option<AuthDirective>, AuthError>;

    async fn is_authenticated(&self, storage: Arc<dyn AuthStorage>) -> Result<bool, AuthError>;

    fn model_visibility(&self) -> Option<Box<dyn ModelVisibilityPolicy>> {
        None
    }
}

pub trait ModelVisibilityPolicy: Send + Sync {
    fn allow_model(&self, model_id: &ModelId, auth_source: &AuthSource) -> bool;
}
