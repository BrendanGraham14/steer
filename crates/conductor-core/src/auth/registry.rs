use crate::api::ProviderKind;
use crate::auth::{AuthFlowWrapper, AuthStorage, DynAuthenticationFlow};
use std::sync::Arc;

/// Registry for creating authentication flows for different providers
pub struct ProviderRegistry;

impl ProviderRegistry {
    pub fn create_auth_flow(
        provider: ProviderKind,
        storage: Arc<dyn AuthStorage>,
    ) -> Option<Box<dyn DynAuthenticationFlow>> {
        match provider {
            ProviderKind::Anthropic => {
                use crate::auth::anthropic::AnthropicOAuthFlow;
                Some(Box::new(AuthFlowWrapper::new(AnthropicOAuthFlow::new(
                    storage,
                ))))
            }
            ProviderKind::OpenAI | ProviderKind::Google | ProviderKind::Grok => {
                use crate::auth::api_key::ApiKeyAuthFlow;
                Some(Box::new(AuthFlowWrapper::new(ApiKeyAuthFlow::new(
                    storage, provider,
                ))))
            }
        }
    }
}
